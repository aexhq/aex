//! The guest-root supervisor: verb dispatch, the start identity table, the cancel
//! ladder and the provider lifecycle hooks.
//!
//! `hands-agent` is a **usability component, not a trust boundary**. It runs as
//! real root because H-BOUNDARY grants the customer real root, and nothing it says
//! is authority. `TerminalMetadata` is customer-controlled observation; not AEX
//! authority. Killing or rewriting the agent fails the customer's own operation and
//! authorises nothing.

use aex_hands_protocol::operation::{
    OperationBounds, OperationExit, OperationFailure, OperationRequest, StopSignal,
    TerminalMetadata, TerminalState,
};
use aex_hands_protocol::rpc::{
    CallHash, CancelReason, CancelResponse, Fence, HandsOperationId, ResultChunk, ResultResponse,
    StartResponse, StatusResponse,
};
use aex_wire::ids::{ContentHash, GenerationId};
use aex_wire::types::Timestamp;

use crate::journal::{Journal, JournalError, OperationMeta};

/// How long a cancelled process group has after `SIGTERM` before `SIGKILL`.
pub const CANCEL_GRACE_MS: u64 = 2_000;

/// How long extinction is polled for after `SIGKILL` before the operation is
/// terminalized anyway.
pub const CANCEL_REAP_MS: u64 = 5_000;

/// How many consecutive empty group scans prove extinction.
pub const EXTINCTION_SCANS: u32 = 3;

/// The minimum interval those scans must span.
pub const EXTINCTION_SCAN_SPAN_MS: u64 = 30;

/// The internal health endpoint every Rust deployable serves.
pub const HEALTHZ_PATH: &str = "/internal/healthz";

/// The internal readiness endpoint.
pub const READYZ_PATH: &str = "/internal/readyz";

/// Why the guest refused to start something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRefusal {
    /// The generation's image carries no such capability. Refused before any
    /// process starts, which is what makes a capability gate meaningful.
    CapabilityUnavailable {
        /// The capability the request needed.
        capability: &'static str,
    },
    /// The concurrency ceiling is reached.
    ConcurrencyExhausted {
        /// How many are open.
        open: u32,
        /// The ceiling.
        limit: u16,
    },
    /// The deadline has already passed.
    DeadlineExpired,
    /// The requested bounds exceed the generation's. A guest may lower a bound and
    /// never raise one.
    BoundsUnsatisfiable {
        /// Which bound.
        bound: &'static str,
    },
}

impl StartRefusal {
    /// The typed failure this refusal becomes on the wire.
    #[must_use]
    pub fn failure(&self) -> OperationFailure {
        match self {
            Self::CapabilityUnavailable { capability } => OperationFailure {
                reason: "capability_unavailable".to_owned(),
                detail: Some((*capability).to_owned()),
                retryable: false,
            },
            Self::ConcurrencyExhausted { open, limit } => OperationFailure {
                reason: "invalid_argument".to_owned(),
                detail: Some(format!(
                    "{open} operations are open; the ceiling is {limit}"
                )),
                retryable: true,
            },
            Self::DeadlineExpired => OperationFailure {
                reason: "timeout".to_owned(),
                detail: Some("the deadline passed before the guest could start".to_owned()),
                retryable: false,
            },
            Self::BoundsUnsatisfiable { bound } => OperationFailure {
                reason: "invalid_argument".to_owned(),
                detail: Some(format!("`{bound}` exceeds the generation bound")),
                retryable: false,
            },
        }
    }
}

/// What `start` decided, before anything is spawned.
///
/// Separating the decision from the effect is the point: a test proves that
/// [`StartDecision::Conflict`] and [`StartDecision::Refused`] start no process by
/// construction, because neither carries anything to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartDecision {
    /// Nothing is recorded: write `meta.json`, fsync, then spawn.
    Spawn {
        /// The record to write before the fork.
        meta: Box<OperationMeta>,
    },
    /// The exact same call is already recorded. No second process, no second side
    /// effect; this is what makes `start` safely repeatable on the cooperative path.
    AlreadyStarted,
    /// The operation exists under a different call hash. Nothing is started.
    Conflict {
        /// The hash the guest already holds.
        recorded_call_hash: CallHash,
    },
    /// The operation already reached a terminal state.
    AlreadyTerminal {
        /// How it ended.
        state: TerminalState,
    },
    /// The guest refuses. Nothing is started.
    Refused(StartRefusal),
}

impl StartDecision {
    /// Whether this decision starts a process.
    #[must_use]
    pub const fn starts_a_process(&self) -> bool {
        matches!(self, Self::Spawn { .. })
    }

    /// The wire response for this decision.
    #[must_use]
    pub fn response(&self, operation: HandsOperationId) -> StartResponse {
        match self {
            Self::Spawn { .. } => StartResponse::Accepted {
                operation,
                existing: false,
            },
            Self::AlreadyStarted => StartResponse::Accepted {
                operation,
                existing: true,
            },
            Self::Conflict { recorded_call_hash } => StartResponse::Conflict {
                operation,
                recorded_call_hash: *recorded_call_hash,
            },
            Self::AlreadyTerminal { state } => StartResponse::AlreadyTerminal {
                operation,
                state: *state,
            },
            Self::Refused(refusal) => StartResponse::Rejected {
                operation,
                failure: refusal.failure(),
            },
        }
    }
}

/// What a `start` asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartInput {
    /// Which operation.
    pub operation: HandsOperationId,
    /// The hash Brain persisted before dispatch.
    pub call_hash: CallHash,
    /// What to do.
    pub request: OperationRequest,
    /// The bounds Brain resolved.
    pub bounds: OperationBounds,
    /// After this instant the guest must not start.
    pub deadline: Timestamp,
    /// How many operations are already open.
    pub open_operations: u32,
    /// Whether the pinned image carries the browser capability.
    pub browser_available: bool,
}

/// The credential-free guest supervisor.
///
/// # Boundary
///
/// This type links no AWS crate, holds no credential and can produce no billable
/// fact. `tests/no_cloud_authority.rs` and `tests/no_guest_billing.rs` fail if
/// either changes.
#[derive(Debug)]
pub struct Supervisor {
    /// The journal.
    journal: Journal,
    /// The exact generation this guest serves.
    generation: GenerationId,
    /// The highest fence observed.
    fence_floor: Fence,
    /// The generation's bounds. A request may lower one, never raise one.
    bounds: OperationBounds,
    /// The incarnation that is serving.
    incarnation: u32,
}

impl Supervisor {
    /// Binds a supervisor to one generation.
    #[must_use]
    pub const fn new(
        journal: Journal,
        generation: GenerationId,
        fence_floor: Fence,
        bounds: OperationBounds,
        incarnation: u32,
    ) -> Self {
        Self {
            journal,
            generation,
            fence_floor,
            bounds,
            incarnation,
        }
    }

    /// The journal.
    #[must_use]
    pub const fn journal(&self) -> &Journal {
        &self.journal
    }

    /// The generation this guest serves.
    #[must_use]
    pub const fn generation(&self) -> GenerationId {
        self.generation
    }

    /// The highest fence observed.
    #[must_use]
    pub const fn fence_floor(&self) -> Fence {
        self.fence_floor
    }

    /// Adopts a higher fence after the bounded request envelope has been checked.
    pub const fn adopt_fence(&mut self, fence: Fence) {
        if fence.0 > self.fence_floor.0 {
            self.fence_floor = fence;
        }
    }

    /// The start identity table.
    ///
    /// | Recorded state | Decision |
    /// | --- | --- |
    /// | absent | `Spawn`, after the metadata fsync |
    /// | same `call_hash`, not terminal | `AlreadyStarted` — no second process |
    /// | different `call_hash` | `Conflict` — no process is started |
    /// | terminal | `AlreadyTerminal` |
    /// | bounds, capability or concurrency refuse | `Refused` — no process |
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when the journal cannot be read. A read failure is
    /// never treated as absence: starting a second process because the journal was
    /// briefly unreadable would double a side effect.
    pub fn decide_start(
        &self,
        input: &StartInput,
        now: Timestamp,
    ) -> Result<StartDecision, JournalError> {
        if let Some(terminal) = self.journal.read_terminal(input.operation)? {
            return Ok(StartDecision::AlreadyTerminal {
                state: terminal.state,
            });
        }
        if let Some(meta) = self.journal.read_meta(input.operation)? {
            return Ok(if meta.call_hash == input.call_hash {
                StartDecision::AlreadyStarted
            } else {
                StartDecision::Conflict {
                    recorded_call_hash: meta.call_hash,
                }
            });
        }
        if let Some(refusal) = self.refuse(input, now) {
            return Ok(StartDecision::Refused(refusal));
        }
        Ok(StartDecision::Spawn {
            meta: Box::new(OperationMeta {
                operation: input.operation,
                call_hash: input.call_hash,
                request: input.request.clone(),
                bounds: self.lower_bounds(&input.bounds),
                deadline: input.deadline,
                started_at: now,
                incarnation: self.incarnation,
            }),
        })
    }

    /// The admission checks, in the order a refusal is reported.
    ///
    /// Capability comes first, deliberately: a capability the image does not carry
    /// must fail before anything else is even considered, so the refusal cannot be
    /// masked by a bound the caller could simply lower and retry.
    fn refuse(&self, input: &StartInput, now: Timestamp) -> Option<StartRefusal> {
        if requires_browser(&input.request) && !input.browser_available {
            return Some(StartRefusal::CapabilityUnavailable {
                capability: "browser",
            });
        }
        if input.deadline < now {
            return Some(StartRefusal::DeadlineExpired);
        }
        if input.bounds.max_output_bytes > self.bounds.max_output_bytes {
            return Some(StartRefusal::BoundsUnsatisfiable {
                bound: "max_output_bytes",
            });
        }
        if input.bounds.max_frame_bytes > self.bounds.max_frame_bytes {
            return Some(StartRefusal::BoundsUnsatisfiable {
                bound: "max_frame_bytes",
            });
        }
        if input.bounds.max_wall_ms > self.bounds.max_wall_ms {
            return Some(StartRefusal::BoundsUnsatisfiable {
                bound: "max_wall_ms",
            });
        }
        let limit = input
            .bounds
            .max_concurrent_operations
            .min(self.bounds.max_concurrent_operations);
        if input.open_operations >= u32::from(limit) {
            return Some(StartRefusal::ConcurrencyExhausted {
                open: input.open_operations,
                limit,
            });
        }
        None
    }

    /// The effective bounds: the pointwise minimum of the request's and the
    /// generation's. A guest may lower a bound and never raise one.
    fn lower_bounds(&self, requested: &OperationBounds) -> OperationBounds {
        OperationBounds {
            max_output_bytes: requested.max_output_bytes.min(self.bounds.max_output_bytes),
            max_frame_bytes: requested.max_frame_bytes.min(self.bounds.max_frame_bytes),
            max_wall_ms: requested.max_wall_ms.min(self.bounds.max_wall_ms),
            max_concurrent_operations: requested
                .max_concurrent_operations
                .min(self.bounds.max_concurrent_operations),
        }
    }

    /// The `status` answer for one operation.
    ///
    /// `status` on an operation the guest has never heard of is the canonical no-op
    /// probe: it returns `Unknown`, costs the guest nothing, and makes liveness a
    /// property of any round trip rather than a sixth verb.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when the journal cannot be read.
    pub fn status(&self, operation: HandsOperationId) -> Result<StatusResponse, JournalError> {
        if let Some(terminal) = self.journal.read_terminal(operation)? {
            return Ok(StatusResponse::Terminal {
                operation,
                terminal,
            });
        }
        let Some(meta) = self.journal.read_meta(operation)? else {
            return Ok(StatusResponse::Unknown { operation });
        };
        if self.journal.read_process(operation)?.is_none() {
            return Ok(StatusResponse::Accepted { operation });
        }
        Ok(StatusResponse::Running {
            operation,
            started_at: meta.started_at,
            phase: None,
            produced_bytes: self.journal.output_len(operation)?,
        })
    }

    /// The `result` answer: a resumable pull over the retained bytes.
    ///
    /// Brain asks only for bytes it has not incorporated, verifies `body_len` and
    /// `digest` over the assembled whole, and only then commits the tool result. A
    /// mux crash mid-pull resumes from the last incorporated offset.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when the journal cannot be read.
    pub fn result(
        &self,
        operation: HandsOperationId,
        from_offset: u64,
        max_bytes: u64,
    ) -> Result<ResultResponse, JournalError> {
        let Some(terminal) = self.journal.read_terminal(operation)? else {
            let state = self.status(operation)?;
            return Ok(match state {
                StatusResponse::Unknown { operation } => ResultResponse::Unknown { operation },
                other => ResultResponse::NotTerminal {
                    operation,
                    state: Box::new(other),
                },
            });
        };
        let retained = self.journal.output_len(operation)?;
        let bytes = self
            .journal
            .read_output(operation, from_offset, max_bytes)?;
        let chunk = if bytes.is_empty() && from_offset >= retained {
            None
        } else {
            let last = from_offset.saturating_add(bytes.len() as u64) >= retained;
            Some(ResultChunk {
                operation,
                offset: from_offset,
                bytes,
                last,
            })
        };
        Ok(ResultResponse::Terminal { terminal, chunk })
    }

    /// The `cancel` answer.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when the journal cannot be read.
    pub fn cancel(
        &self,
        operation: HandsOperationId,
        _reason: CancelReason,
    ) -> Result<CancelResponse, JournalError> {
        if let Some(terminal) = self.journal.read_terminal(operation)? {
            return Ok(CancelResponse::AlreadyTerminal {
                operation,
                terminal,
            });
        }
        if self.journal.read_meta(operation)?.is_none() {
            return Ok(CancelResponse::Unknown { operation });
        }
        Ok(CancelResponse::Cancelling { operation })
    }
}

/// Whether an operation needs the browser capability.
///
/// This stays the single place the gate is evaluated, and it is evaluated before
/// anything is spawned. The exhaustive match that forces a new arm to declare
/// which side of the gate it is on now lives on the contract type, so the two
/// cannot disagree about what needs a browser.
///
/// `TODO(cross-stream)`: the browser executor itself is still absent; the gate
/// rejects every `Browser` operation with `capability_unavailable` until it
/// lands. Owner: hands.
const fn requires_browser(request: &OperationRequest) -> bool {
    request.requires_browser()
}

/// One rung of the cancel ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelStep {
    /// Send this signal to the operation's process **group**.
    Signal(StopSignal),
    /// Keep polling for the group's extinction.
    Reap,
    /// Terminalize as cancelled.
    ///
    /// `escaped` is `true` when the group is still alive at the reap deadline,
    /// which means something double-forked out of it. That is reported honestly as
    /// best effort rather than claimed as a clean kill.
    Terminalize {
        /// Whether anything escaped the process group.
        escaped: bool,
    },
}

/// The cancel ladder as a pure function of elapsed time and liveness.
///
/// `SIGTERM`, then [`CANCEL_GRACE_MS`], then `SIGKILL`, then extinction polling
/// for at most [`CANCEL_REAP_MS`], then terminalize.
#[must_use]
pub const fn cancel_step(elapsed_ms: u64, group_alive: bool) -> CancelStep {
    if !group_alive {
        return CancelStep::Terminalize { escaped: false };
    }
    if elapsed_ms < CANCEL_GRACE_MS {
        CancelStep::Signal(StopSignal::Term)
    } else if elapsed_ms == CANCEL_GRACE_MS {
        CancelStep::Signal(StopSignal::Kill)
    } else if elapsed_ms < CANCEL_GRACE_MS + CANCEL_REAP_MS {
        CancelStep::Reap
    } else {
        CancelStep::Terminalize { escaped: true }
    }
}

/// The `blake3` of the empty body.
#[must_use]
pub fn empty_digest() -> ContentHash {
    ContentHash::from_bytes(*blake3::hash(b"").as_bytes())
}

/// The terminal record replay writes for an operation whose substrate vanished.
#[must_use]
pub fn interrupted_terminal(meta: &OperationMeta, now: Timestamp) -> TerminalMetadata {
    TerminalMetadata {
        state: TerminalState::Interrupted,
        exit: OperationExit::Timeout,
        started_at: meta.started_at,
        ended_at: now,
        body_len: 0,
        digest: empty_digest(),
        truncated: false,
        result_file: None,
        failure: Some(OperationFailure {
            reason: "guest_interrupted".to_owned(),
            detail: Some("the supervisor restarted with no live process group".to_owned()),
            retryable: true,
        }),
    }
}

/// Which provider lifecycle hook is being served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleHook {
    /// First boot of a generation.
    Run,
    /// Restore from a snapshot.
    Resume,
    /// About to snapshot.
    Suspend,
    /// About to be destroyed.
    Terminate,
    /// Build hook: is the rootfs contract satisfied.
    Ready,
    /// Build hook: does the installed package set match the lockfile.
    Validate,
}

impl LifecycleHook {
    /// Every hook.
    pub const ALL: [Self; 6] = [
        Self::Run,
        Self::Resume,
        Self::Suspend,
        Self::Terminate,
        Self::Ready,
        Self::Validate,
    ];

    /// The path the provider posts to.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Run => "/aws/lambda-microvms/runtime/v1/run",
            Self::Resume => "/aws/lambda-microvms/runtime/v1/resume",
            Self::Suspend => "/aws/lambda-microvms/runtime/v1/suspend",
            Self::Terminate => "/aws/lambda-microvms/runtime/v1/terminate",
            Self::Ready => "/aws/lambda-microvms/runtime/v1/ready",
            Self::Validate => "/aws/lambda-microvms/runtime/v1/validate",
        }
    }

    /// Whether the guest accepts operations after this hook succeeds.
    #[must_use]
    pub const fn opens_admission(self) -> bool {
        matches!(self, Self::Run | Self::Resume)
    }

    /// Whether the hook must bump the incarnation counter.
    #[must_use]
    pub const fn bumps_incarnation(self) -> bool {
        matches!(self, Self::Run | Self::Resume)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CANCEL_GRACE_MS, CANCEL_REAP_MS, CancelStep, LifecycleHook, StartDecision, StartInput,
        StartRefusal, Supervisor, cancel_step, empty_digest, interrupted_terminal,
    };
    use crate::journal::{
        Journal, JournalError, OperationMeta, ProcessProbe, ProcessRecord, ReplayVerdict,
    };
    use aex_hands_protocol::operation::{
        FileMode, GuestPath, GuestRoot, OperationBounds, OperationExit, OperationRequest,
        StopSignal, TerminalMetadata, TerminalState,
    };
    use aex_hands_protocol::rpc::{
        CallHash, CancelReason, CancelResponse, Fence, HandsOperationId, ResultResponse,
        StartResponse, StatusResponse,
    };
    use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn operation(tag: u64) -> HandsOperationId {
        HandsOperationId(Uuid7::compose(
            tag,
            [u8::try_from(tag & 0xff).unwrap_or(0); 10],
        ))
    }

    fn call_hash(tag: u8) -> CallHash {
        CallHash(ContentHash::from_bytes([tag; 32]))
    }

    fn bounds() -> OperationBounds {
        OperationBounds {
            max_output_bytes: 1_000_000,
            max_frame_bytes: 1_048_576,
            max_wall_ms: 600_000,
            max_concurrent_operations: 32,
        }
    }

    fn request() -> OperationRequest {
        OperationRequest::WriteFile {
            path: GuestPath::parse(&GuestRoot::workspace(), "/workspace/out.txt")
                .expect("a contained path"),
            mode: FileMode::ReadWrite,
            content: b"new".to_vec(),
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        supervisor: Supervisor,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("a temporary journal root");
        let journal = Journal::open(dir.path()).expect("the journal opens");
        let supervisor = Supervisor::new(
            journal,
            GenerationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            Fence(0),
            bounds(),
            1,
        );
        Fixture {
            _dir: dir,
            supervisor,
        }
    }

    fn input(tag: u64, hash: u8) -> StartInput {
        StartInput {
            operation: operation(tag),
            call_hash: call_hash(hash),
            request: request(),
            bounds: bounds(),
            deadline: at(1_000_000),
            open_operations: 0,
            browser_available: false,
        }
    }

    fn terminal(state: TerminalState) -> TerminalMetadata {
        TerminalMetadata {
            state,
            exit: OperationExit::Ok,
            started_at: at(0),
            ended_at: at(1),
            body_len: 0,
            digest: empty_digest(),
            truncated: false,
            result_file: None,
            failure: None,
        }
    }

    fn spawn(fixture: &Fixture, input: &StartInput) -> Box<OperationMeta> {
        let StartDecision::Spawn { meta } = fixture
            .supervisor
            .decide_start(input, at(0))
            .expect("the journal reads")
        else {
            panic!("a first start spawns");
        };
        fixture
            .supervisor
            .journal()
            .record_start(&meta)
            .expect("the record lands");
        meta
    }

    #[test]
    fn a_first_start_spawns_and_a_replay_of_the_same_call_does_not() {
        let fixture = fixture();
        let input = input(1, 7);
        let _ = spawn(&fixture, &input);

        let second = fixture
            .supervisor
            .decide_start(&input, at(0))
            .expect("the journal reads");
        assert_eq!(second, StartDecision::AlreadyStarted);
        assert!(
            !second.starts_a_process(),
            "a replay must not produce a second side effect"
        );
        assert_eq!(
            second.response(input.operation),
            StartResponse::Accepted {
                operation: input.operation,
                existing: true
            }
        );
    }

    #[test]
    fn a_different_call_hash_conflicts_and_starts_nothing() {
        let fixture = fixture();
        let _ = spawn(&fixture, &input(1, 7));

        let forged = input(1, 8);
        let decision = fixture
            .supervisor
            .decide_start(&forged, at(0))
            .expect("the journal reads");
        assert_eq!(
            decision,
            StartDecision::Conflict {
                recorded_call_hash: call_hash(7)
            }
        );
        assert!(!decision.starts_a_process());
    }

    #[test]
    fn a_terminal_operation_is_never_restarted() {
        let fixture = fixture();
        let input = input(1, 7);
        fixture
            .supervisor
            .journal()
            .record_terminal(input.operation, &terminal(TerminalState::Succeeded))
            .expect("the terminal lands");
        let decision = fixture
            .supervisor
            .decide_start(&input, at(0))
            .expect("the journal reads");
        assert_eq!(
            decision,
            StartDecision::AlreadyTerminal {
                state: TerminalState::Succeeded
            }
        );
        assert!(!decision.starts_a_process());
    }

    #[test]
    fn every_refusal_starts_no_process_and_writes_no_metadata() {
        let fixture = fixture();
        let cases: [(StartInput, StartRefusal); 3] = [
            (
                StartInput {
                    deadline: at(-1),
                    ..input(2, 1)
                },
                StartRefusal::DeadlineExpired,
            ),
            (
                StartInput {
                    bounds: OperationBounds {
                        max_output_bytes: u64::MAX,
                        ..bounds()
                    },
                    ..input(3, 1)
                },
                StartRefusal::BoundsUnsatisfiable {
                    bound: "max_output_bytes",
                },
            ),
            (
                StartInput {
                    open_operations: 32,
                    ..input(4, 1)
                },
                StartRefusal::ConcurrencyExhausted {
                    open: 32,
                    limit: 32,
                },
            ),
        ];
        for (probe, expected) in cases {
            let decision = fixture
                .supervisor
                .decide_start(&probe, at(0))
                .expect("the journal reads");
            assert_eq!(decision, StartDecision::Refused(expected.clone()));
            assert!(
                !decision.starts_a_process(),
                "{expected:?} must start no process"
            );
            assert!(
                fixture
                    .supervisor
                    .journal()
                    .read_meta(probe.operation)
                    .expect("the journal reads")
                    .is_none(),
                "a refusal writes no metadata"
            );
        }
    }

    #[test]
    fn a_bound_may_be_lowered_and_never_raised() {
        let fixture = fixture();
        let lowered = StartInput {
            bounds: OperationBounds {
                max_output_bytes: 10,
                max_frame_bytes: 64,
                max_wall_ms: 5,
                max_concurrent_operations: 1,
            },
            ..input(5, 1)
        };
        let StartDecision::Spawn { meta } = fixture
            .supervisor
            .decide_start(&lowered, at(0))
            .expect("the journal reads")
        else {
            panic!("a lowered bound is accepted");
        };
        assert_eq!(meta.bounds.max_output_bytes, 10);
        assert_eq!(meta.bounds.max_frame_bytes, 64);
        assert_eq!(meta.bounds.max_wall_ms, 5);
        assert_eq!(meta.bounds.max_concurrent_operations, 1);
    }

    #[test]
    fn the_cancel_ladder_is_term_then_kill_then_reap_then_an_honest_terminal() {
        assert_eq!(cancel_step(0, true), CancelStep::Signal(StopSignal::Term));
        assert_eq!(
            cancel_step(CANCEL_GRACE_MS - 1, true),
            CancelStep::Signal(StopSignal::Term)
        );
        assert_eq!(
            cancel_step(CANCEL_GRACE_MS, true),
            CancelStep::Signal(StopSignal::Kill),
            "the grace boundary is exact"
        );
        assert_eq!(cancel_step(CANCEL_GRACE_MS + 1, true), CancelStep::Reap);
        assert_eq!(
            cancel_step(CANCEL_GRACE_MS + CANCEL_REAP_MS - 1, true),
            CancelStep::Reap
        );
        assert_eq!(
            cancel_step(CANCEL_GRACE_MS + CANCEL_REAP_MS, true),
            CancelStep::Terminalize { escaped: true },
            "a group still alive at the reap deadline escaped, and that is reported"
        );
        assert_eq!(
            cancel_step(0, false),
            CancelStep::Terminalize { escaped: false },
            "a group that is already gone terminalizes immediately"
        );
    }

    #[test]
    fn cancel_answers_unknown_cancelling_and_already_terminal() {
        let fixture = fixture();
        assert_eq!(
            fixture
                .supervisor
                .cancel(operation(9), CancelReason::CustomerStop)
                .expect("the journal reads"),
            CancelResponse::Unknown {
                operation: operation(9)
            }
        );

        let input = input(1, 7);
        let _ = spawn(&fixture, &input);
        assert_eq!(
            fixture
                .supervisor
                .cancel(input.operation, CancelReason::CustomerStop)
                .expect("the journal reads"),
            CancelResponse::Cancelling {
                operation: input.operation
            }
        );

        fixture
            .supervisor
            .journal()
            .record_terminal(input.operation, &terminal(TerminalState::Cancelled))
            .expect("the terminal lands");
        assert!(matches!(
            fixture
                .supervisor
                .cancel(input.operation, CancelReason::CustomerStop)
                .expect("the journal reads"),
            CancelResponse::AlreadyTerminal { .. }
        ));
    }

    #[test]
    fn an_unknown_operation_is_the_canonical_no_op_probe() {
        let fixture = fixture();
        let probe = operation(0);
        assert_eq!(
            fixture.supervisor.status(probe).expect("the journal reads"),
            StatusResponse::Unknown { operation: probe }
        );
    }

    #[test]
    fn status_moves_from_accepted_to_running_to_terminal() {
        let fixture = fixture();
        let input = input(1, 7);
        let _ = spawn(&fixture, &input);
        assert!(matches!(
            fixture
                .supervisor
                .status(input.operation)
                .expect("the journal reads"),
            StatusResponse::Accepted { .. }
        ));

        fixture
            .supervisor
            .journal()
            .record_process(
                input.operation,
                ProcessRecord {
                    pgid: 4_242,
                    start_time: 99,
                },
            )
            .expect("the record lands");
        fixture
            .supervisor
            .journal()
            .append_output(input.operation, b"hello")
            .expect("the append lands");
        let StatusResponse::Running { produced_bytes, .. } = fixture
            .supervisor
            .status(input.operation)
            .expect("the journal reads")
        else {
            panic!("a spawned operation is running");
        };
        assert_eq!(produced_bytes, 5);

        fixture
            .supervisor
            .journal()
            .record_terminal(input.operation, &terminal(TerminalState::Succeeded))
            .expect("the terminal lands");
        assert!(matches!(
            fixture
                .supervisor
                .status(input.operation)
                .expect("the journal reads"),
            StatusResponse::Terminal { .. }
        ));
    }

    #[test]
    fn a_result_pull_resumes_from_an_arbitrary_offset_and_reports_the_last_chunk() {
        let fixture = fixture();
        let operation = operation(1);
        fixture
            .supervisor
            .journal()
            .append_output(operation, b"0123456789")
            .expect("the append lands");
        fixture
            .supervisor
            .journal()
            .record_terminal(operation, &terminal(TerminalState::Succeeded))
            .expect("the terminal lands");

        let ResultResponse::Terminal { chunk, .. } = fixture
            .supervisor
            .result(operation, 0, 4)
            .expect("the journal reads")
        else {
            panic!("a terminal operation pulls");
        };
        let chunk = chunk.expect("bytes exist");
        assert_eq!(chunk.bytes, b"0123");
        assert!(!chunk.last);

        let ResultResponse::Terminal { chunk, .. } = fixture
            .supervisor
            .result(operation, 4, 64)
            .expect("the journal reads")
        else {
            panic!("a terminal operation pulls");
        };
        let chunk = chunk.expect("bytes exist");
        assert_eq!(chunk.offset, 4);
        assert_eq!(chunk.bytes, b"456789");
        assert!(chunk.last);

        let ResultResponse::Terminal { chunk, .. } = fixture
            .supervisor
            .result(operation, 10, 64)
            .expect("the journal reads")
        else {
            panic!("a terminal operation pulls");
        };
        assert!(chunk.is_none(), "asking past the end is not an error");
    }

    #[test]
    fn a_non_terminal_result_pull_returns_the_state_and_no_body() {
        let fixture = fixture();
        let input = input(1, 7);
        let _ = spawn(&fixture, &input);
        assert!(matches!(
            fixture
                .supervisor
                .result(input.operation, 0, 64)
                .expect("the journal reads"),
            ResultResponse::NotTerminal { .. }
        ));
        assert!(matches!(
            fixture
                .supervisor
                .result(operation(77), 0, 64)
                .expect("the journal reads"),
            ResultResponse::Unknown { .. }
        ));
    }

    struct Probe(Option<u64>);

    impl ProcessProbe for Probe {
        fn leader_start_time(&self, _pgid: i32) -> Result<Option<u64>, String> {
            Ok(self.0)
        }
    }

    struct BrokenProbe;

    impl ProcessProbe for BrokenProbe {
        fn leader_start_time(&self, _pgid: i32) -> Result<Option<u64>, String> {
            Err("/proc is unreadable".to_owned())
        }
    }

    #[test]
    fn replay_interrupts_a_crash_between_the_metadata_fsync_and_the_fork() {
        let fixture = fixture();
        // The crash injection: the metadata is fsynced and nothing is spawned.
        let _ = spawn(&fixture, &input(1, 7));

        let entries = fixture
            .supervisor
            .journal()
            .replay(&Probe(None))
            .expect("replay reads");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].verdict, ReplayVerdict::Interrupted);
    }

    #[test]
    fn replay_keeps_a_live_group_and_refuses_a_recycled_pid() {
        let fixture = fixture();
        let input = input(1, 7);
        let _ = spawn(&fixture, &input);
        let record = ProcessRecord {
            pgid: 1_234,
            start_time: 55_555,
        };
        fixture
            .supervisor
            .journal()
            .record_process(input.operation, record)
            .expect("the record lands");

        let alive = fixture
            .supervisor
            .journal()
            .replay(&Probe(Some(55_555)))
            .expect("replay reads");
        assert_eq!(alive[0].verdict, ReplayVerdict::StillRunning(record));

        // The same pid, a different start time: an unrelated recycled pid, which a
        // bare pid check would have adopted.
        let recycled = fixture
            .supervisor
            .journal()
            .replay(&Probe(Some(99_999)))
            .expect("replay reads");
        assert_eq!(recycled[0].verdict, ReplayVerdict::Interrupted);
    }

    #[test]
    fn a_failed_probe_is_never_collapsed_into_extinction() {
        let fixture = fixture();
        let input = input(1, 7);
        let _ = spawn(&fixture, &input);
        fixture
            .supervisor
            .journal()
            .record_process(
                input.operation,
                ProcessRecord {
                    pgid: 1_234,
                    start_time: 55_555,
                },
            )
            .expect("the record lands");
        let outcome = fixture.supervisor.journal().replay(&BrokenProbe);
        assert!(
            matches!(outcome, Err(JournalError::Malformed { .. })),
            "an unreadable probe must surface, not terminalize a live job"
        );
    }

    #[test]
    fn replay_keeps_a_terminal_record_exactly_as_it_stands() {
        let fixture = fixture();
        let input = input(1, 7);
        let _ = spawn(&fixture, &input);
        fixture
            .supervisor
            .journal()
            .record_terminal(input.operation, &terminal(TerminalState::Succeeded))
            .expect("the terminal lands");

        let entries = fixture
            .supervisor
            .journal()
            .replay(&Probe(None))
            .expect("replay reads");
        assert!(matches!(entries[0].verdict, ReplayVerdict::Terminal(_)));
    }

    #[test]
    fn a_terminal_record_is_written_exactly_once() {
        let fixture = fixture();
        let operation = operation(1);
        fixture
            .supervisor
            .journal()
            .record_terminal(operation, &terminal(TerminalState::Succeeded))
            .expect("the first terminal lands");
        let second = fixture
            .supervisor
            .journal()
            .record_terminal(operation, &terminal(TerminalState::Failed));
        assert!(
            matches!(second, Err(JournalError::AlreadyTerminal { .. })),
            "a second terminal write is a guest bug, not a silent overwrite"
        );
    }

    #[test]
    fn a_rewritten_journal_record_is_refused_rather_than_repaired() {
        let fixture = fixture();
        let input = input(1, 7);
        let _ = spawn(&fixture, &input);
        let path = fixture
            .supervisor
            .journal()
            .operation_dir(input.operation)
            .join("meta.json");
        std::fs::write(&path, b"{ customer tampering }").expect("the tamper lands");
        let outcome = fixture.supervisor.decide_start(&input, at(0));
        assert!(
            matches!(outcome, Err(JournalError::Malformed { .. })),
            "guessing at a rewritten journal would be inventing state"
        );
    }

    #[test]
    fn the_incarnation_counter_advances_on_every_run_and_resume() {
        let dir = tempfile::tempdir().expect("a temporary journal root");
        let journal = Journal::open(dir.path()).expect("the journal opens");
        assert_eq!(journal.incarnation().expect("the counter reads"), 0);
        assert_eq!(journal.bump_incarnation().expect("the counter bumps"), 1);
        assert_eq!(journal.bump_incarnation().expect("the counter bumps"), 2);
        assert_eq!(journal.incarnation().expect("the counter reads"), 2);

        for hook in LifecycleHook::ALL {
            assert_eq!(
                hook.bumps_incarnation(),
                matches!(hook, LifecycleHook::Run | LifecycleHook::Resume),
                "{hook:?}"
            );
            assert_eq!(hook.opens_admission(), hook.bumps_incarnation(), "{hook:?}");
        }
    }

    #[test]
    fn every_lifecycle_hook_has_the_provider_path_the_image_declares() {
        assert_eq!(
            LifecycleHook::ALL.map(LifecycleHook::path),
            [
                "/aws/lambda-microvms/runtime/v1/run",
                "/aws/lambda-microvms/runtime/v1/resume",
                "/aws/lambda-microvms/runtime/v1/suspend",
                "/aws/lambda-microvms/runtime/v1/terminate",
                "/aws/lambda-microvms/runtime/v1/ready",
                "/aws/lambda-microvms/runtime/v1/validate",
            ]
        );
    }

    #[test]
    fn a_suspend_flush_syncs_every_open_operation() {
        let fixture = fixture();
        let _ = spawn(&fixture, &input(1, 7));
        fixture
            .supervisor
            .journal()
            .flush_all()
            .expect("the flush succeeds");
    }

    #[test]
    fn an_interrupted_terminal_names_the_guest_interruption_and_journals_no_body() {
        let fixture = fixture();
        let input = input(1, 7);
        let meta = spawn(&fixture, &input);
        let terminal = interrupted_terminal(&meta, at(500));
        assert_eq!(terminal.state, TerminalState::Interrupted);
        assert_eq!(terminal.body_len, 0);
        assert_eq!(
            terminal
                .failure
                .as_ref()
                .map(|failure| failure.reason.as_str()),
            Some("guest_interrupted")
        );
    }

    #[test]
    fn a_higher_fence_is_adopted_and_a_lower_one_is_not() {
        let mut fixture = fixture();
        fixture.supervisor.adopt_fence(Fence(5));
        assert_eq!(fixture.supervisor.fence_floor(), Fence(5));
        fixture.supervisor.adopt_fence(Fence(2));
        assert_eq!(
            fixture.supervisor.fence_floor(),
            Fence(5),
            "the floor never moves backwards"
        );
    }
}
