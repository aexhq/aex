//! The on-disk operation journal.
//!
//! ```text
//! /var/lib/aex/hands/
//!   incarnation                     u32 LE, fsynced, incremented on every /run and /resume
//!   binding.json                    generation, fence floor, protocol version, root, bounds
//!   operations/<operation_id>/
//!     meta.json                     operation id, call_hash, request, bounds, deadline, started_at
//!     out.bin                       captured stdout+stderr, interleaved, bounded
//!     pid                           process group id and its start time
//!     terminal.json                 written once, fsynced, then the directory fsynced
//! ```
//!
//! Three ordering rules, each with a crash-injection test:
//!
//! 1. `meta.json` is written and fsynced, and its parent directory fsynced,
//!    **before** any process is spawned. A crash after the fsync and before the
//!    spawn is observed on replay as a started operation with no live process,
//!    which is `Interrupted`.
//! 2. `out.bin` is append-only, and the terminal digest is over the **retained**
//!    bytes, so truncation is honest rather than a digest failure.
//! 3. `terminal.json` is created exclusively. A second write is a guest bug and
//!    surfaces as `AlreadyTerminal`.
//!
//! The journal is a **cooperative cache, not an authority**. Customer root can
//! delete or rewrite any of it; doing so fails that customer's own operation and
//! authorises nothing. Everything recorded here is customer-controlled
//! observation; not AEX authority.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};

use aex_hands_protocol::operation::{OperationBounds, OperationRequest, TerminalMetadata};
use aex_hands_protocol::rpc::{CallHash, Fence, HandsOperationId};
use aex_internal_contracts::SchemaVersion;
use aex_wire::ids::GenerationId;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

/// Whether this host can fsync a directory.
///
/// The guest target is `aarch64-unknown-linux-musl`, where it always can. On a
/// non-POSIX development host the directory half of ordering rule 1 cannot be
/// exercised, and this constant says so out loud rather than letting a green run
/// imply coverage it did not have.
pub const DIRECTORY_SYNC_AVAILABLE: bool = cfg!(unix);

/// Why a journal operation failed.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// The underlying filesystem refused.
    #[error("journal I/O failed at `{path}`: {source}")]
    Io {
        /// Which path.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// A journal record did not decode. Never repaired in place: a rewritten
    /// journal is customer tampering, and guessing at it would be inventing state.
    #[error("journal record `{path}` is malformed: {reason}")]
    Malformed {
        /// Which path.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// A terminal record already exists.
    #[error("operation {operation} is already terminal")]
    AlreadyTerminal {
        /// Which operation.
        operation: String,
    },
}

/// Wraps an I/O error with the path that produced it.
fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> JournalError + use<'_> {
    move |source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// What the guest was launched with. Written once by the `/run` hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GuestBinding {
    /// The exact generation this guest serves, and only this one.
    pub generation: GenerationId,
    /// The highest fence observed so far. Adopted upward, never downward.
    pub fence_floor: Fence,
    /// The exact protocol version. No negotiation.
    pub protocol_version: SchemaVersion,
    /// The guest filesystem root.
    pub root: String,
    /// Bounds the guest may lower but never raise.
    pub bounds: OperationBounds,
}

/// The per-operation record written before anything is spawned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationMeta {
    /// Which operation.
    pub operation: HandsOperationId,
    /// The hash Brain persisted before dispatch.
    pub call_hash: CallHash,
    /// What was asked for.
    pub request: OperationRequest,
    /// Bounds for this operation.
    pub bounds: OperationBounds,
    /// After this instant the guest must not start.
    pub deadline: Timestamp,
    /// When the record was written.
    pub started_at: Timestamp,
    /// The incarnation that wrote it.
    pub incarnation: u32,
}

/// The recorded process group, with the start time that makes a pid check sound.
///
/// A bare pid check would adopt an unrelated recycled pid on replay, which is how
/// a supervisor ends up reporting a stranger's process as the customer's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProcessRecord {
    /// The process group leader.
    pub pgid: i32,
    /// `/proc/<pid>/stat` field 22, the leader's start time in clock ticks.
    pub start_time: u64,
}

/// What replay found for one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayVerdict {
    /// Already terminal; the record stands and the body is still pullable.
    Terminal(Box<TerminalMetadata>),
    /// A live process group whose leader pid *and* start time both match.
    StillRunning(ProcessRecord),
    /// Started, but nothing is running under it any more.
    Interrupted,
}

/// One operation as replay sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayEntry {
    /// Which operation.
    pub operation: HandsOperationId,
    /// The recorded metadata.
    pub meta: Box<OperationMeta>,
    /// What replay decided.
    pub verdict: ReplayVerdict,
}

/// Answers "is this recorded process group still the one we started".
///
/// A port rather than a direct `/proc` read, so the whole replay matrix — pid
/// recycling included, which cannot be produced on demand — runs off-VM.
pub trait ProcessProbe {
    /// The start time of the process group leader, or `None` when no such process
    /// exists.
    ///
    /// # Errors
    ///
    /// Returns a message when the probe itself failed. That is not the same as the
    /// process being absent and must never be collapsed into it: treating an
    /// unreadable probe as extinction would terminalize a live job.
    fn leader_start_time(&self, pgid: i32) -> Result<Option<u64>, String>;
}

/// The on-disk journal.
#[derive(Debug, Clone)]
pub struct Journal {
    /// The journal root, `/var/lib/aex/hands` in the guest.
    root: PathBuf,
}

impl Journal {
    /// Opens, creating the directory tree if it is absent.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] when the tree cannot be created.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let root = root.into();
        let operations = root.join("operations");
        std::fs::create_dir_all(&operations).map_err(io_at(&operations))?;
        sync_directory(&root)?;
        Ok(Self { root })
    }

    /// The journal root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory one operation owns.
    #[must_use]
    pub fn operation_dir(&self, operation: HandsOperationId) -> PathBuf {
        self.root.join("operations").join(operation.0.to_string())
    }

    /// Increments and returns the incarnation counter.
    ///
    /// Called on every `/run` and `/resume` and stamped into every response
    /// preamble, so Brain can tell "the same live process" from "the supervisor
    /// restarted under it" without trusting the guest's story about which.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on a read or write failure and
    /// [`JournalError::Malformed`] when the counter file is not four bytes.
    pub fn bump_incarnation(&self) -> Result<u32, JournalError> {
        let next = self.incarnation()?.saturating_add(1);
        let path = self.root.join("incarnation");
        write_synced(&path, &next.to_le_bytes())?;
        sync_directory(&self.root)?;
        Ok(next)
    }

    /// The current incarnation without advancing it.
    ///
    /// # Errors
    ///
    /// See [`Journal::bump_incarnation`].
    pub fn incarnation(&self) -> Result<u32, JournalError> {
        let path = self.root.join("incarnation");
        match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == 4 => {
                Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            }
            Ok(bytes) => Err(JournalError::Malformed {
                path,
                reason: format!("the incarnation counter is 4 bytes, found {}", bytes.len()),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(io_at(&path)(error)),
        }
    }

    /// Writes the launch binding.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on a write failure.
    pub fn write_binding(&self, binding: &GuestBinding) -> Result<(), JournalError> {
        let path = self.root.join("binding.json");
        write_json(&path, binding)?;
        sync_directory(&self.root)
    }

    /// Reads the launch binding, if the guest has one.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Malformed`] when the record does not decode.
    pub fn read_binding(&self) -> Result<Option<GuestBinding>, JournalError> {
        read_json(&self.root.join("binding.json"))
    }

    /// Reads one operation's metadata, if the guest has it.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Malformed`] when the record does not decode.
    pub fn read_meta(
        &self,
        operation: HandsOperationId,
    ) -> Result<Option<OperationMeta>, JournalError> {
        read_json(&self.operation_dir(operation).join("meta.json"))
    }

    /// Ordering rule 1: writes and fsyncs `meta.json`, then fsyncs its parent
    /// directory, and only then returns so the caller may spawn.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on any write or sync failure. A failure here
    /// must abort the start: spawning a process the journal does not know about is
    /// how an operation becomes unaddressable.
    pub fn record_start(&self, meta: &OperationMeta) -> Result<(), JournalError> {
        let dir = self.operation_dir(meta.operation);
        std::fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        write_json(&dir.join("meta.json"), meta)?;
        sync_directory(&dir)
    }

    /// Records the spawned process group and its start time.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on a write failure.
    pub fn record_process(
        &self,
        operation: HandsOperationId,
        record: ProcessRecord,
    ) -> Result<(), JournalError> {
        let dir = self.operation_dir(operation);
        write_json(&dir.join("pid"), &record)?;
        sync_directory(&dir)
    }

    /// The recorded process group, if one was spawned.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Malformed`] when the record does not decode.
    pub fn read_process(
        &self,
        operation: HandsOperationId,
    ) -> Result<Option<ProcessRecord>, JournalError> {
        read_json(&self.operation_dir(operation).join("pid"))
    }

    /// Ordering rule 3: creates `terminal.json` exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::AlreadyTerminal`] when a terminal record already
    /// exists. The create is exclusive, so the exclusion is the filesystem's and
    /// not a read-then-write race.
    pub fn record_terminal(
        &self,
        operation: HandsOperationId,
        terminal: &TerminalMetadata,
    ) -> Result<(), JournalError> {
        let dir = self.operation_dir(operation);
        std::fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        let path = dir.join("terminal.json");
        let encoded = encode(&path, terminal)?;
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(JournalError::AlreadyTerminal {
                    operation: operation.0.to_string(),
                });
            }
            Err(error) => return Err(io_at(&path)(error)),
        };
        file.write_all(&encoded).map_err(io_at(&path))?;
        file.sync_all().map_err(io_at(&path))?;
        sync_directory(&dir)
    }

    /// The terminal record, if the operation has one.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Malformed`] when the record does not decode.
    pub fn read_terminal(
        &self,
        operation: HandsOperationId,
    ) -> Result<Option<TerminalMetadata>, JournalError> {
        read_json(&self.operation_dir(operation).join("terminal.json"))
    }

    /// Appends captured output. Rule 2: append-only, never rewritten.
    ///
    /// Returns the retained length after the append.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on a write failure.
    pub fn append_output(
        &self,
        operation: HandsOperationId,
        bytes: &[u8],
    ) -> Result<u64, JournalError> {
        let dir = self.operation_dir(operation);
        std::fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        let path = dir.join("out.bin");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_at(&path))?;
        file.write_all(bytes).map_err(io_at(&path))?;
        file.flush().map_err(io_at(&path))?;
        file.metadata().map(|meta| meta.len()).map_err(io_at(&path))
    }

    /// Reads a bounded window of captured output, which is what a resumable pull
    /// asks for.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on a read failure. Asking past the end yields
    /// an empty window rather than an error: Brain resumes from its own last
    /// incorporated offset and may legitimately hold all of it already.
    pub fn read_output(
        &self,
        operation: HandsOperationId,
        from_offset: u64,
        max_bytes: u64,
    ) -> Result<Vec<u8>, JournalError> {
        let path = self.operation_dir(operation).join("out.bin");
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_at(&path)(error)),
        };
        let len = file.metadata().map_err(io_at(&path))?.len();
        if from_offset >= len {
            return Ok(Vec::new());
        }
        file.seek(std::io::SeekFrom::Start(from_offset))
            .map_err(io_at(&path))?;
        let want = usize::try_from((len - from_offset).min(max_bytes)).unwrap_or(usize::MAX);
        let mut out = vec![0u8; want];
        file.read_exact(&mut out).map_err(io_at(&path))?;
        Ok(out)
    }

    /// How many output bytes are retained for an operation.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on a stat failure.
    pub fn output_len(&self, operation: HandsOperationId) -> Result<u64, JournalError> {
        let path = self.operation_dir(operation).join("out.bin");
        match std::fs::metadata(&path) {
            Ok(meta) => Ok(meta.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(io_at(&path)(error)),
        }
    }

    /// Fsyncs every operation directory, for the `/suspend` hook.
    ///
    /// Nothing is persisted remotely. The journal dies with the VM; the flush
    /// exists so a *suspend* resumes onto a clean tree.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::Io`] on a sync failure.
    pub fn flush_all(&self) -> Result<(), JournalError> {
        let operations = self.root.join("operations");
        for entry in children(&operations)? {
            sync_directory(&entry)?;
        }
        sync_directory(&operations)?;
        sync_directory(&self.root)
    }

    /// Replays the journal, classifying every recorded operation.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when the tree cannot be read or a record does not
    /// decode.
    pub fn replay(&self, probe: &dyn ProcessProbe) -> Result<Vec<ReplayEntry>, JournalError> {
        let operations = self.root.join("operations");
        let mut entries = Vec::new();
        for dir in children(&operations)? {
            let meta_path = dir.join("meta.json");
            let Some(meta) = read_json::<OperationMeta>(&meta_path)? else {
                // A directory with no metadata was never started: there is nothing
                // to classify and nothing to invent.
                continue;
            };
            let verdict = if let Some(terminal) = self.read_terminal(meta.operation)? {
                ReplayVerdict::Terminal(Box::new(terminal))
            } else {
                self.classify_live(meta.operation, probe)?
            };
            entries.push(ReplayEntry {
                operation: meta.operation,
                meta: Box::new(meta),
                verdict,
            });
        }
        entries.sort_by_key(|entry| entry.operation.0);
        Ok(entries)
    }

    /// Decides whether a non-terminal operation still has its own process group.
    fn classify_live(
        &self,
        operation: HandsOperationId,
        probe: &dyn ProcessProbe,
    ) -> Result<ReplayVerdict, JournalError> {
        let Some(record) = self.read_process(operation)? else {
            // meta.json exists but nothing was ever spawned: a crash between the
            // fsync and the fork.
            return Ok(ReplayVerdict::Interrupted);
        };
        let observed =
            probe
                .leader_start_time(record.pgid)
                .map_err(|reason| JournalError::Malformed {
                    path: self.operation_dir(operation).join("pid"),
                    reason,
                })?;
        match observed {
            // Both the pid and its start time must match. A bare pid check would
            // adopt an unrelated recycled pid.
            Some(start_time) if start_time == record.start_time => {
                Ok(ReplayVerdict::StillRunning(record))
            }
            _ => Ok(ReplayVerdict::Interrupted),
        }
    }
}

/// Lists a directory's immediate children, sorted, tolerating absence.
fn children(path: &Path) -> Result<Vec<PathBuf>, JournalError> {
    let mut out = Vec::new();
    let iter = match std::fs::read_dir(path) {
        Ok(iter) => iter,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(io_at(path)(error)),
    };
    for entry in iter {
        out.push(entry.map_err(io_at(path))?.path());
    }
    out.sort();
    Ok(out)
}

/// Encodes a record, naming the path in any failure.
fn encode<T: Serialize>(path: &Path, value: &T) -> Result<Vec<u8>, JournalError> {
    serde_json::to_vec(value).map_err(|error| JournalError::Malformed {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Writes a JSON record and fsyncs it.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), JournalError> {
    let encoded = encode(path, value)?;
    write_synced(path, &encoded)
}

/// Reads a JSON record, tolerating absence and refusing to repair a bad one.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, JournalError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| JournalError::Malformed {
                    path: path.to_path_buf(),
                    reason: error.to_string(),
                })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_at(path)(error)),
    }
}

/// Writes a whole file and fsyncs it.
fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), JournalError> {
    let mut file = File::create(path).map_err(io_at(path))?;
    file.write_all(bytes).map_err(io_at(path))?;
    file.sync_all().map_err(io_at(path))
}

/// Fsyncs a directory so a create or rename is durable.
///
/// On a host where a directory cannot be opened as a file this is a no-op, and
/// [`DIRECTORY_SYNC_AVAILABLE`] is `false` so the gap is visible rather than
/// implied. The guest target is POSIX and always takes the real path.
fn sync_directory(path: &Path) -> Result<(), JournalError> {
    if !DIRECTORY_SYNC_AVAILABLE {
        return Ok(());
    }
    File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(io_at(path))
}
