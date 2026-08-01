//! The operation dispatcher.
//!
//! One path for every operation: bound the output, append it to the journal, and
//! write exactly one terminal record. `Exec` is the only guest primitive that
//! starts a process; `git`, `package_install` and `code_run` are Brain-side argv
//! constructors over it, so there is one process primitive to audit.
//!
//! Everything reported here is **customer-controlled observation; not AEX
//! authority**. Brain bounds, digests and stores it as tool output.

use std::sync::Arc;

use aex_hands_agent::capture::Capture;
use aex_hands_agent::journal::{Journal, OperationMeta, ProcessRecord};
use aex_hands_agent::session::empty_digest;
use aex_hands_protocol::operation::{
    DeliveryMode, GuestPath, GuestProcessId, GuestRoot, OperationExit, OperationFailure,
    OperationRequest, TerminalMetadata, TerminalState,
};
use aex_hands_protocol::rpc::{HandsOperationId, OutputStream};
use aex_hands_tools::command::{build_env, build_spawn};
use aex_hands_tools::filesystem;
use aex_hands_tools::observation::{self, SearchCandidate};
use aex_hands_tools::port::{EntryKind, FsError, GuestFs as _, Pgid};
use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;

use crate::host::{HostFs, OutputSink, Runner};

/// How deep a recursive listing descends.
pub const LIST_DEPTH: u32 = 2;

/// The operation a background-process identity names.
///
/// `GuestProcessId` is the `HandsOperationId` of the operation that started the
/// process, spelled as text on the wire. A value that is not one names no
/// operation this guest ever started, and is reported as `not_found` rather than
/// guessed at.
fn operation_of(process: &GuestProcessId) -> Option<HandsOperationId> {
    serde_json::from_value::<HandsOperationId>(serde_json::Value::String(process.0.clone())).ok()
}

/// What a dispatch produced.
#[derive(Debug)]
pub enum Dispatch {
    /// The operation is finished and this is its one terminal record.
    Terminal(Box<TerminalMetadata>),
    /// A background process group was started. The terminal record is written
    /// when it is reaped.
    Started {
        /// The recorded group.
        record: ProcessRecord,
    },
}

/// Appends captured bytes to one operation's journal, within its bounds.
struct JournalSink<'a> {
    journal: &'a Journal,
    operation: HandsOperationId,
    capture: Capture,
    truncated: bool,
    produced: u64,
}

impl OutputSink for JournalSink<'_> {
    fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.produced = self.produced.saturating_add(bytes.len() as u64);
        let outcome = self.capture.write(OutputStream::Stdout, bytes);
        if outcome.retain.len() < bytes.len() {
            self.truncated = true;
        }
        if outcome.retain.is_empty() {
            return Ok(());
        }
        self.journal
            .append_output(self.operation, &outcome.retain)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// The guest's operation dispatcher.
pub struct Executor {
    /// The real filesystem.
    fs: HostFs,
    /// The real process host.
    runner: Arc<dyn Runner>,
    /// The guest root every structured tool stays inside.
    root: GuestRoot,
}

impl core::fmt::Debug for Executor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Executor")
            .field("root", &self.root.0)
            .finish_non_exhaustive()
    }
}

impl Executor {
    /// Composes a dispatcher.
    #[must_use]
    pub fn new(runner: Arc<dyn Runner>, root: GuestRoot) -> Self {
        Self {
            fs: HostFs::new(root.clone(), &root.0),
            runner,
            root,
        }
    }

    /// The guest root.
    #[must_use]
    pub const fn root(&self) -> &GuestRoot {
        &self.root
    }

    /// The guest root as a path, for the environment builder.
    fn root_path(&self) -> Result<GuestPath, String> {
        GuestPath::parse(&self.root, &self.root.0).map_err(|error| error.to_string())
    }

    /// Every file under `root`, bounded, for a search.
    ///
    /// The walk is bounded rather than exhaustive: a customer with root can put
    /// anything under the workspace, and an unbounded walk is a denial of service
    /// against the guest's own supervisor.
    fn candidates(&self, root: &GuestPath) -> Result<Vec<SearchCandidate>, FsError> {
        let mut out = Vec::new();
        let mut frontier = vec![root.clone()];
        while let Some(current) = frontier.pop() {
            if out.len() >= observation::SEARCH_MAX_FILES {
                break;
            }
            for entry in self.fs.read_dir(&current)? {
                let text = format!("{}/{}", current.as_str(), entry.name);
                let Ok(child) = GuestPath::parse(&self.root, &text) else {
                    continue;
                };
                match entry.meta.kind {
                    // `.git` is listed and never descended, and a symlink is never
                    // followed: following one would let a link inside the workspace
                    // pull in the rest of the filesystem.
                    EntryKind::Directory if entry.name != ".git" => frontier.push(child),
                    EntryKind::File => out.push(SearchCandidate {
                        relative_path: text
                            .strip_prefix(root.as_str())
                            .unwrap_or(&text)
                            .trim_start_matches('/')
                            .to_owned(),
                        bytes: self.fs.read(&child)?,
                    }),
                    EntryKind::Directory | EntryKind::Symlink | EntryKind::Other => {}
                }
            }
        }
        Ok(out)
    }

    /// The process host, for the cancel ladder.
    #[must_use]
    pub fn runner(&self) -> &Arc<dyn Runner> {
        &self.runner
    }

    /// Runs one operation.
    ///
    /// # Errors
    ///
    /// Returns the journal error when the journal itself refused. A journal
    /// failure is never reported as an operation failure: the customer's command
    /// may well have run.
    pub fn dispatch(
        &self,
        journal: &Journal,
        meta: &OperationMeta,
        delivery: DeliveryMode,
        now: Timestamp,
    ) -> Result<Dispatch, aex_hands_agent::journal::JournalError> {
        match &meta.request {
            OperationRequest::Exec { .. } => self.dispatch_exec(journal, meta, delivery, now),
            OperationRequest::ProcessStop { .. } | OperationRequest::ProcessStatus { .. } => {
                self.dispatch_process(journal, meta, now)
            }
            _ => self.dispatch_workspace(journal, meta, now),
        }
    }

    /// The one guest primitive that starts a process.
    fn dispatch_exec(
        &self,
        journal: &Journal,
        meta: &OperationMeta,
        delivery: DeliveryMode,
        now: Timestamp,
    ) -> Result<Dispatch, aex_hands_agent::journal::JournalError> {
        match &meta.request {
            OperationRequest::Exec {
                argv,
                cwd,
                env,
                stdin,
            } => {
                let inherited = crate::host::inherited_environment();
                let root_path = match self.root_path() {
                    Ok(path) => path,
                    Err(reason) => {
                        return Ok(Dispatch::Terminal(Box::new(failed(
                            meta,
                            now,
                            "invalid_argument",
                            &reason,
                        ))));
                    }
                };
                let environment = match build_env(&inherited, env, &root_path) {
                    Ok(environment) => environment,
                    Err(error) => {
                        return Ok(Dispatch::Terminal(Box::new(failed(
                            meta,
                            now,
                            "invalid_argument",
                            &error.to_string(),
                        ))));
                    }
                };
                if stdin.is_some() {
                    return Ok(Dispatch::Terminal(Box::new(failed(
                        meta,
                        now,
                        "capability_unavailable",
                        UNRESOLVABLE_CONTENT,
                    ))));
                }
                let spec = match build_spawn(argv, cwd.clone(), environment, None) {
                    Ok(spec) => spec,
                    Err(error) => {
                        return Ok(Dispatch::Terminal(Box::new(failed(
                            meta,
                            now,
                            "invalid_argument",
                            &error.to_string(),
                        ))));
                    }
                };
                let started = match self.runner.start(&spec) {
                    Ok(started) => started,
                    Err(error) => {
                        return Ok(Dispatch::Terminal(Box::new(failed(
                            meta,
                            now,
                            error.code(),
                            &error.to_string(),
                        ))));
                    }
                };
                let record = ProcessRecord {
                    pgid: started.pgid.0,
                    start_time: started.start_time,
                };
                journal.record_process(meta.operation, record)?;
                if delivery == DeliveryMode::Detached {
                    return Ok(Dispatch::Started { record });
                }
                let mut sink = JournalSink {
                    journal,
                    operation: meta.operation,
                    capture: Capture::new(&meta.bounds, delivery == DeliveryMode::Attached),
                    truncated: false,
                    produced: 0,
                };
                let exit = (started.reap)(&mut sink);
                Ok(Dispatch::Terminal(Box::new(Self::terminal(
                    journal, meta, now, exit, &sink,
                )?)))
            }
            _ => unreachable!("dispatch routes only Exec here"),
        }
    }

    /// The two background-process arms, both answered from the journal.
    fn dispatch_process(
        &self,
        journal: &Journal,
        meta: &OperationMeta,
        now: Timestamp,
    ) -> Result<Dispatch, aex_hands_agent::journal::JournalError> {
        match &meta.request {
            OperationRequest::ProcessStop { process, signal } => {
                let group = match operation_of(process) {
                    Some(operation) => journal
                        .read_process(operation)?
                        .map(|record| Pgid(record.pgid)),
                    None => None,
                };
                let Some(group) = group else {
                    return Ok(Dispatch::Terminal(Box::new(failed(
                        meta,
                        now,
                        "not_found",
                        "no process group is recorded for that operation",
                    ))));
                };
                match self.runner.signal(group, *signal) {
                    Ok(()) => {
                        let extinct = self.runner.alive(group).is_ok_and(|alive| !alive);
                        Self::text_terminal(
                            journal,
                            meta,
                            now,
                            &format!("{{\"signalled\":true,\"extinct\":{extinct}}}"),
                        )
                    }
                    Err(error) => Ok(Dispatch::Terminal(Box::new(failed(
                        meta,
                        now,
                        error.code(),
                        &error.to_string(),
                    )))),
                }
            }
            OperationRequest::ProcessStatus {
                process,
                from_offset,
                max_bytes,
            } => {
                let Some(operation) = operation_of(process) else {
                    return Ok(Dispatch::Terminal(Box::new(failed(
                        meta,
                        now,
                        "not_found",
                        "that process identity names no operation this guest started",
                    ))));
                };
                let window = journal.read_output(operation, *from_offset, u64::from(*max_bytes))?;
                let text = String::from_utf8_lossy(&window).into_owned();
                Self::text_terminal(journal, meta, now, &text)
            }
            _ => unreachable!("dispatch routes only the process arms here"),
        }
    }

    /// The workspace arms: the structured file tools, and the ones that need
    /// something this guest deliberately does not carry.
    fn dispatch_workspace(
        &self,
        journal: &Journal,
        meta: &OperationMeta,
        now: Timestamp,
    ) -> Result<Dispatch, aex_hands_agent::journal::JournalError> {
        match &meta.request {
            OperationRequest::ReadFile { path, range } => {
                if range.is_some() {
                    // The wire asks for a byte range; the executor windows by line.
                    // Serving the whole file instead would answer a different
                    // question than the caller asked, so the mismatch is refused
                    // and recorded rather than papered over.
                    return Ok(Dispatch::Terminal(Box::new(failed(
                        meta,
                        now,
                        "invalid_argument",
                        "the wire read range is a byte range and the guest executor windows by \
                         line; see the recorded gap",
                    ))));
                }
                match filesystem::read_file(&self.fs, path, None) {
                    Ok(outcome) => Self::text_terminal(journal, meta, now, &outcome.text),
                    Err(error) => Ok(Dispatch::Terminal(Box::new(failed(
                        meta,
                        now,
                        error.code(),
                        &error.to_string(),
                    )))),
                }
            }
            OperationRequest::StatPath { path } => match filesystem::stat_path(&self.fs, path) {
                Ok(text) => Self::text_terminal(journal, meta, now, &text),
                Err(error) => Ok(Dispatch::Terminal(Box::new(failed(
                    meta,
                    now,
                    error.code(),
                    &error.to_string(),
                )))),
            },
            OperationRequest::ListDir {
                path,
                recursive,
                limit,
            } => match filesystem::list_dir(
                &self.fs,
                &self.root,
                path,
                *recursive,
                LIST_DEPTH,
                usize::try_from(*limit).unwrap_or(usize::MAX),
            ) {
                Ok(outcome) => Self::text_terminal(journal, meta, now, &outcome.lines.join("\n")),
                Err(error) => Ok(Dispatch::Terminal(Box::new(failed(
                    meta,
                    now,
                    error.code(),
                    &error.to_string(),
                )))),
            },
            OperationRequest::Search { .. }
            | OperationRequest::Materialize { .. }
            | OperationRequest::Persist { .. }
            | OperationRequest::Browser { .. }
            | OperationRequest::RegisteredTool { .. } => {
                self.dispatch_search_or_refuse(journal, meta, now)
            }
            OperationRequest::Exec { .. }
            | OperationRequest::ProcessStop { .. }
            | OperationRequest::ProcessStatus { .. }
            | OperationRequest::WriteFile { .. }
            | OperationRequest::EditFile { .. } => {
                unreachable!("dispatch routes those arms elsewhere")
            }
        }
    }

    /// The search executor, and the arms this guest deliberately refuses.
    fn dispatch_search_or_refuse(
        &self,
        journal: &Journal,
        meta: &OperationMeta,
        now: Timestamp,
    ) -> Result<Dispatch, aex_hands_agent::journal::JournalError> {
        match &meta.request {
            OperationRequest::Search {
                root,
                pattern,
                limit,
            } => {
                let candidates = match self.candidates(root) {
                    Ok(candidates) => candidates,
                    Err(error) => {
                        return Ok(Dispatch::Terminal(Box::new(failed(
                            meta,
                            now,
                            error.code(),
                            &error.to_string(),
                        ))));
                    }
                };
                // The deadline is a pure input to the executor, so the search
                // matrix stays deterministic. The guest supplies no elapsed
                // milliseconds because the walk above is already bounded by the
                // file cap.
                let outcome = observation::search(&candidates, pattern, *limit, u64::MAX, &|_| 0);
                let mut text = outcome.matches.join("\n");
                if let Some(notice) = outcome.notice {
                    text.push('\n');
                    text.push_str(&notice);
                }
                Self::text_terminal(journal, meta, now, &text)
            }
            OperationRequest::WriteFile {
                path,
                mode,
                content,
            } => {
                let _ = (path, mode, content);
                Ok(Dispatch::Terminal(Box::new(failed(
                    meta,
                    now,
                    "capability_unavailable",
                    UNRESOLVABLE_CONTENT,
                ))))
            }
            OperationRequest::EditFile {
                path,
                expected,
                patch,
            } => match filesystem::edit_file(&self.fs, path, *expected, patch) {
                Ok(outcome) => Self::text_terminal(
                    journal,
                    meta,
                    now,
                    &format!("{} ({:?})", outcome.digest, outcome.applied),
                ),
                Err(error) => Ok(Dispatch::Terminal(Box::new(failed(
                    meta,
                    now,
                    edit_code(&error),
                    &error.to_string(),
                )))),
            },
            // Each of these needs something this guest deliberately does not carry:
            // an HTTPS client for the presigned workspace grants, headless Chromium
            // for the browser capability, and the registered-tool manifest
            // resolver. Every one fails closed with the reason named, before
            // anything is spawned.
            OperationRequest::Materialize { .. } | OperationRequest::Persist { .. } => {
                Ok(Dispatch::Terminal(Box::new(failed(
                    meta,
                    now,
                    "capability_unavailable",
                    "workspace materialize and persist run over presigned HTTPS, which needs a \
                     TLS client the guest does not carry; see the recorded gap",
                ))))
            }
            OperationRequest::Browser { .. } => Ok(Dispatch::Terminal(Box::new(failed(
                meta,
                now,
                "capability_unavailable",
                "browser",
            )))),
            OperationRequest::RegisteredTool { .. } => Ok(Dispatch::Terminal(Box::new(failed(
                meta,
                now,
                "capability_unavailable",
                "a registered tool is resolved by Brain and dispatched as an Exec",
            )))),
            _ => unreachable!("dispatch routes the structured file arms elsewhere"),
        }
    }

    /// Writes a text body into the journal and returns its terminal record.
    fn text_terminal(
        journal: &Journal,
        meta: &OperationMeta,
        now: Timestamp,
        text: &str,
    ) -> Result<Dispatch, aex_hands_agent::journal::JournalError> {
        let mut sink = JournalSink {
            journal,
            operation: meta.operation,
            capture: Capture::new(&meta.bounds, false),
            truncated: false,
            produced: 0,
        };
        let stored = sink.append(text.as_bytes()).is_ok();
        let exit = if stored {
            Ok(OperationExit::Ok)
        } else {
            Ok(OperationExit::NonZero { code: 1 })
        };
        Ok(Dispatch::Terminal(Box::new(Self::terminal(
            journal, meta, now, exit, &sink,
        )?)))
    }

    /// The one terminal record an operation gets.
    ///
    /// The digest is over the **retained** bytes, so truncation is honest rather
    /// than a digest failure.
    fn terminal(
        journal: &Journal,
        meta: &OperationMeta,
        now: Timestamp,
        exit: Result<OperationExit, aex_hands_tools::port::ProcError>,
        sink: &JournalSink<'_>,
    ) -> Result<TerminalMetadata, aex_hands_agent::journal::JournalError> {
        let retained = journal.read_output(meta.operation, 0, u64::MAX)?;
        let digest = if retained.is_empty() {
            empty_digest()
        } else {
            ContentHash::from_bytes(*blake3::hash(&retained).as_bytes())
        };
        let (state, exit, failure) = match exit {
            Ok(OperationExit::Ok) => (TerminalState::Succeeded, OperationExit::Ok, None),
            Ok(other) => (
                TerminalState::Failed,
                other,
                Some(OperationFailure {
                    reason: "invalid_argument".to_owned(),
                    detail: Some("the command exited non-zero".to_owned()),
                    retryable: false,
                }),
            ),
            Err(error) => (
                TerminalState::Failed,
                OperationExit::NonZero { code: -1 },
                Some(OperationFailure {
                    reason: error.code().to_owned(),
                    detail: Some(error.to_string()),
                    retryable: false,
                }),
            ),
        };
        Ok(TerminalMetadata {
            state,
            exit,
            started_at: meta.started_at,
            ended_at: now,
            body_len: retained.len() as u64,
            digest,
            truncated: sink.truncated,
            failure,
        })
    }
}

/// The stable failure code an edit failure is reported as.
const fn edit_code(error: &filesystem::EditError) -> &'static str {
    match error {
        filesystem::EditError::StaleWrite { .. } => "stale_write",
        filesystem::EditError::NotFound { .. }
        | filesystem::EditError::Ambiguous { .. }
        | filesystem::EditError::PatchTooLarge { .. } => "invalid_argument",
        filesystem::EditError::Fs(inner) => inner.code(),
    }
}

/// Why a content reference cannot be resolved inside the guest.
///
/// `ContentRef` is a digest and a length. The guest holds no credential, so it
/// cannot turn one into bytes; the contract change that gives these arms a
/// presigned plan URL is a recorded gap. Refusing is the only honest answer —
/// writing an empty file, or running a command with empty standard input, would
/// silently do something the caller did not ask for.
const UNRESOLVABLE_CONTENT: &str = "the guest holds no credential and cannot resolve a content digest to bytes; the arm needs \
     the presigned plan the contract gap describes";

/// A terminal record for an operation that failed before it produced anything.
fn failed(meta: &OperationMeta, now: Timestamp, reason: &str, detail: &str) -> TerminalMetadata {
    TerminalMetadata {
        state: TerminalState::Failed,
        exit: OperationExit::NonZero { code: -1 },
        started_at: meta.started_at,
        ended_at: now,
        body_len: 0,
        digest: empty_digest(),
        truncated: false,
        failure: Some(OperationFailure {
            reason: reason.to_owned(),
            detail: Some(detail.to_owned()),
            retryable: false,
        }),
    }
}
