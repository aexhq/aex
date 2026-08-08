//! The cancel ladder driver.
//!
//! [`aex_hands_agent::session::cancel_step`] is the pure decision table:
//! `SIGTERM`, then the grace, then `SIGKILL`, then extinction polling, then an
//! honest terminal. This module is the driver that table never had — a guest
//! background thread that walks the ladder against the real clock and records
//! the `Cancelled` terminal. Without it, a cancel sent one polite `SIGTERM`
//! and nothing ever escalated or terminalized.
//!
//! The driver and the detached reap thread can both reach the terminal write.
//! The `cancel.json` marker, written before the first signal, is how they
//! agree: a reap that finds it records `Cancelled` too, and the exclusive
//! terminal create makes whichever record lands second a tolerated no-op.

use aex_hands_agent::journal::{Journal, JournalError, OperationMeta};
use aex_hands_agent::session::{CancelStep, cancel_step, empty_digest};
use aex_hands_protocol::operation::{
    OperationExit, OperationFailure, StopSignal, TerminalMetadata, TerminalState,
};
use aex_hands_tools::port::Pgid;
use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;

use crate::host::Runner;

/// How often the driver re-examines the group between ladder rungs.
pub const CANCEL_POLL_MS: u64 = 100;

/// Drives the cancel ladder for one operation until it terminalizes.
///
/// `pace` pauses for the given milliseconds and returns the total elapsed
/// milliseconds since the drive began. It is injected so the whole ladder —
/// the escalation, the reap window and the escape verdict — runs deterministically
/// in test; production passes a real sleep over a monotonic clock.
///
/// # Errors
///
/// Returns [`JournalError`] when the terminal record cannot be written. An
/// already-terminal operation is not an error: the reap thread honoured the
/// cancel marker first and its record stands.
pub fn drive(
    runner: &dyn Runner,
    journal: &Journal,
    meta: &OperationMeta,
    group: Pgid,
    pace: &mut dyn FnMut(u64) -> u64,
) -> Result<CancelStep, JournalError> {
    let mut elapsed = 0_u64;
    let mut termed = false;
    let mut killed = false;
    loop {
        // A probe failure is never collapsed into extinction: an unreadable
        // `/proc` must not terminalize a live group as cleanly killed.
        let alive = runner.alive(group).unwrap_or(true);
        let step = cancel_step(elapsed, alive);
        match step {
            CancelStep::Signal(StopSignal::Term | StopSignal::Interrupt) => {
                if !termed {
                    let _ = runner.signal(group, StopSignal::Term);
                    termed = true;
                }
            }
            CancelStep::Signal(StopSignal::Kill) => {
                if !killed {
                    let _ = runner.signal(group, StopSignal::Kill);
                    killed = true;
                }
            }
            CancelStep::Reap => {
                // The `SIGKILL` rung is one exact instant in the table, and a
                // real clock lands beside it, not on it. Crossing the grace
                // boundary between polls must still escalate exactly once.
                if !killed {
                    let _ = runner.signal(group, StopSignal::Kill);
                    killed = true;
                }
            }
            CancelStep::Terminalize { escaped } => {
                // The reap thread is usually the one that lands the terminal:
                // the kill closes the pipes, so it finishes before the next
                // extinction poll. When the driver lands first, `body_len`
                // can trail the last pipe flush by a few bytes — bounded to
                // diagnostics, because Brain never incorporates a cancelled
                // body as a tool result.
                let detail = escaped.then(|| {
                    "the process group outlived SIGKILL through the reap window; something \
                     double-forked out of it and the kill is best effort, not clean"
                        .to_owned()
                });
                let exit = OperationExit::Signal {
                    name: if killed { "SIGKILL" } else { "SIGTERM" }.to_owned(),
                };
                let terminal =
                    cancelled_terminal(journal, meta, crate::host::now(), exit, false, detail)?;
                match journal.record_terminal(meta.operation, &terminal) {
                    Ok(()) | Err(JournalError::AlreadyTerminal { .. }) => {}
                    Err(error) => return Err(error),
                }
                return Ok(step);
            }
        }
        elapsed = pace(CANCEL_POLL_MS);
    }
}

/// The terminal record a cancelled operation gets.
///
/// The digest is over the bytes the journal retained, so a partial output is
/// still verifiable. `truncated` is whatever the writer knows: the reap thread
/// carries the capture's exact state, the driver can only claim what it reads.
///
/// # Errors
///
/// Returns [`JournalError`] when the retained output cannot be read.
pub fn cancelled_terminal(
    journal: &Journal,
    meta: &OperationMeta,
    ended_at: Timestamp,
    exit: OperationExit,
    truncated: bool,
    detail: Option<String>,
) -> Result<TerminalMetadata, JournalError> {
    let retained = journal.read_output(meta.operation, 0, u64::MAX)?;
    let digest = if retained.is_empty() {
        empty_digest()
    } else {
        ContentHash::from_bytes(*blake3::hash(&retained).as_bytes())
    };
    Ok(TerminalMetadata {
        state: TerminalState::Cancelled,
        exit,
        started_at: meta.started_at,
        ended_at,
        body_len: retained.len() as u64,
        digest,
        truncated,
        failure: Some(OperationFailure {
            reason: "cancelled".to_owned(),
            detail,
            retryable: false,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::{CANCEL_POLL_MS, drive};
    use crate::host::{Runner, Started};
    use aex_hands_agent::journal::{Journal, OperationMeta};
    use aex_hands_agent::session::{CANCEL_GRACE_MS, CANCEL_REAP_MS, CancelStep};
    use aex_hands_protocol::operation::{
        GuestPath, GuestRoot, OperationBounds, OperationExit, OperationRequest, StopSignal,
        TerminalState,
    };
    use aex_hands_protocol::rpc::{CallHash, HandsOperationId};
    use aex_hands_tools::port::{Pgid, ProcError};
    use aex_wire::ids::{ContentHash, Uuid7};
    use aex_wire::types::Timestamp;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A group that ignores `SIGTERM` and dies only on `SIGKILL`.
    #[derive(Default)]
    struct StubbornRunner {
        signalled: Mutex<Vec<StopSignal>>,
        dead: AtomicBool,
        /// When set, the group survives even `SIGKILL`: the double-fork escape.
        immortal: bool,
    }

    impl Runner for StubbornRunner {
        fn start(&self, _spec: &aex_hands_tools::command::SpawnSpec) -> Result<Started, ProcError> {
            Err(ProcError::Other(
                "the ladder never starts anything".to_owned(),
            ))
        }

        fn signal(&self, _group: Pgid, signal: StopSignal) -> Result<(), ProcError> {
            self.signalled
                .lock()
                .expect("the fixture lock is not poisoned")
                .push(signal);
            if signal == StopSignal::Kill && !self.immortal {
                self.dead.store(true, Ordering::SeqCst);
            }
            Ok(())
        }

        fn alive(&self, _group: Pgid) -> Result<bool, ProcError> {
            Ok(!self.dead.load(Ordering::SeqCst))
        }
    }

    /// A group that dies on the first signal of any kind.
    struct ObedientRunner(StubbornRunner);

    impl Runner for ObedientRunner {
        fn start(&self, spec: &aex_hands_tools::command::SpawnSpec) -> Result<Started, ProcError> {
            self.0.start(spec)
        }

        fn signal(&self, group: Pgid, signal: StopSignal) -> Result<(), ProcError> {
            self.0.signal(group, signal)?;
            self.0.dead.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn alive(&self, group: Pgid) -> Result<bool, ProcError> {
            self.0.alive(group)
        }
    }

    fn meta(dir: &std::path::Path) -> (Journal, OperationMeta) {
        let journal = Journal::open(dir).expect("the journal opens");
        let meta = OperationMeta {
            operation: HandsOperationId(Uuid7::compose(9, [6; 10])),
            call_hash: CallHash(ContentHash::from_bytes([1; 32])),
            request: OperationRequest::Exec {
                argv: vec!["/bin/sleep".to_owned()],
                cwd: GuestPath::parse(&GuestRoot::workspace(), "/workspace")
                    .expect("a contained path"),
                env: Vec::new(),
                stdin: None,
            },
            bounds: OperationBounds {
                max_output_bytes: 1_000_000,
                max_frame_bytes: 1_048_576,
                max_wall_ms: 600_000,
                max_concurrent_operations: 32,
            },
            deadline: Timestamp::from_unix_millis(4_102_444_800_000).expect("a bounded instant"),
            started_at: Timestamp::from_unix_millis(1_000).expect("a bounded instant"),
            incarnation: 1,
        };
        journal.record_start(&meta).expect("the record lands");
        (journal, meta)
    }

    #[test]
    fn a_term_ignoring_group_is_killed_and_terminalizes_cancelled() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (journal, meta) = meta(dir.path());
        let runner = StubbornRunner::default();
        let mut elapsed = 0_u64;
        let mut pace = |millis: u64| {
            elapsed += millis;
            elapsed
        };
        let step =
            drive(&runner, &journal, &meta, Pgid(42), &mut pace).expect("the ladder terminalizes");
        assert_eq!(step, CancelStep::Terminalize { escaped: false });
        assert_eq!(
            *runner
                .signalled
                .lock()
                .expect("the fixture lock is not poisoned"),
            vec![StopSignal::Term, StopSignal::Kill],
            "one polite TERM, one KILL, nothing repeated"
        );
        let terminal = journal
            .read_terminal(meta.operation)
            .expect("the journal reads")
            .expect("the ladder recorded a terminal");
        assert_eq!(terminal.state, TerminalState::Cancelled);
        assert_eq!(
            terminal.exit,
            OperationExit::Signal {
                name: "SIGKILL".to_owned()
            }
        );
    }

    #[test]
    fn a_poll_that_skips_the_exact_grace_instant_still_escalates_exactly_once() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (journal, meta) = meta(dir.path());
        let runner = StubbornRunner::default();
        // 300 ms steps never land on the 2000 ms rung; the driver must still kill.
        let mut elapsed = 0_u64;
        let mut pace = |_millis: u64| {
            elapsed += 300;
            elapsed
        };
        let step =
            drive(&runner, &journal, &meta, Pgid(42), &mut pace).expect("the ladder terminalizes");
        assert_eq!(step, CancelStep::Terminalize { escaped: false });
        assert_eq!(
            *runner
                .signalled
                .lock()
                .expect("the fixture lock is not poisoned"),
            vec![StopSignal::Term, StopSignal::Kill]
        );
    }

    #[test]
    fn a_group_that_survives_the_kill_is_reported_escaped_and_still_cancelled() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (journal, meta) = meta(dir.path());
        let runner = StubbornRunner {
            immortal: true,
            ..StubbornRunner::default()
        };
        let mut elapsed = 0_u64;
        let mut pace = |millis: u64| {
            elapsed += millis;
            elapsed
        };
        let step =
            drive(&runner, &journal, &meta, Pgid(42), &mut pace).expect("the ladder terminalizes");
        assert_eq!(step, CancelStep::Terminalize { escaped: true });
        assert!(
            elapsed >= CANCEL_GRACE_MS + CANCEL_REAP_MS,
            "the escape verdict is only reached at the reap deadline: {elapsed}"
        );
        let terminal = journal
            .read_terminal(meta.operation)
            .expect("the journal reads")
            .expect("the ladder recorded a terminal");
        assert_eq!(terminal.state, TerminalState::Cancelled);
        assert!(
            terminal
                .failure
                .as_ref()
                .and_then(|failure| failure.detail.as_deref())
                .is_some_and(|detail| detail.contains("best effort")),
            "an escape is reported honestly, never claimed as a clean kill"
        );
    }

    #[test]
    fn an_obedient_group_terminalizes_after_the_polite_term_alone() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (journal, meta) = meta(dir.path());
        let runner = ObedientRunner(StubbornRunner::default());
        let mut elapsed = 0_u64;
        let mut pace = |millis: u64| {
            elapsed += millis;
            elapsed
        };
        let step =
            drive(&runner, &journal, &meta, Pgid(42), &mut pace).expect("the ladder terminalizes");
        assert_eq!(step, CancelStep::Terminalize { escaped: false });
        assert_eq!(
            *runner
                .0
                .signalled
                .lock()
                .expect("the fixture lock is not poisoned"),
            vec![StopSignal::Term],
            "a group that honours SIGTERM is never killed"
        );
        assert!(
            elapsed <= CANCEL_POLL_MS,
            "extinction is seen on the next poll"
        );
        let terminal = journal
            .read_terminal(meta.operation)
            .expect("the journal reads")
            .expect("the ladder recorded a terminal");
        assert_eq!(
            terminal.exit,
            OperationExit::Signal {
                name: "SIGTERM".to_owned()
            }
        );
    }
}
