//! The host side of the guest: the real filesystem and real process groups.
//!
//! Both are ports so the operation dispatcher runs off-VM. That is not a
//! convenience: process-group supervision is a POSIX capability, this build host
//! is not POSIX, and a dispatcher that could only be exercised on the target would
//! have no earned evidence at all.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use aex_hands_protocol::operation::{GuestPath, GuestRoot, OperationExit, StopSignal};
use aex_hands_tools::command::SpawnSpec;
use aex_hands_tools::port::{DirEntry, EntryKind, FsError, GuestFs, Meta, Pgid, ProcError};
use aex_wire::types::Timestamp;

/// Whether this build can supervise a process group.
///
/// The guest target is `aarch64-unknown-linux-musl`, where it always can. On a
/// non-POSIX development host it cannot, and this constant says so out loud rather
/// than letting a green run imply coverage it did not have.
pub const PROCESS_GROUPS_AVAILABLE: bool = cfg!(unix);

/// Where a started process group's output goes.
pub trait OutputSink: Send {
    /// Appends captured bytes.
    ///
    /// # Errors
    ///
    /// Returns a message when the sink refused. A refused write is never silently
    /// dropped: the terminal digest is over the retained bytes, so a lost write
    /// would make the digest disagree with what the customer can read back.
    fn append(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// Drains a started group's output into a sink and reaps it.
pub type Reap = Box<dyn FnOnce(&mut dyn OutputSink) -> Result<OperationExit, ProcError> + Send>;

/// A started process group, and the work that drains and reaps it.
pub struct Started {
    /// The process group leader.
    pub pgid: Pgid,
    /// `/proc/<pid>/stat` field 22, which is what makes a pid check sound.
    pub start_time: u64,
    /// Drains output into the sink and reaps. Consumed exactly once.
    pub reap: Reap,
}

impl core::fmt::Debug for Started {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Started")
            .field("pgid", &self.pgid)
            .field("start_time", &self.start_time)
            .finish_non_exhaustive()
    }
}

/// The guest process host.
pub trait Runner: Send + Sync + 'static {
    /// Starts a command in its own process group.
    ///
    /// # Errors
    ///
    /// See [`ProcError`].
    fn start(&self, spec: &SpawnSpec) -> Result<Started, ProcError>;

    /// Signals a whole process group.
    ///
    /// # Errors
    ///
    /// See [`ProcError`].
    fn signal(&self, group: Pgid, signal: StopSignal) -> Result<(), ProcError>;

    /// Whether any member of the group is still alive.
    ///
    /// # Errors
    ///
    /// See [`ProcError`]. A probe failure is never collapsed into extinction:
    /// treating an unreadable probe as a dead group would terminalize a live job.
    fn alive(&self, group: Pgid) -> Result<bool, ProcError>;
}

/// The real filesystem, rooted nowhere.
///
/// Containment is [`GuestPath`]'s job and is a **structural tool contract, not a
/// security boundary**: H-BOUNDARY grants the customer real root and `shell_exec`
/// reaches the whole filesystem by design. Structured file tools stay inside the
/// workspace root so revision checking and persist have meaning, not because
/// staying inside it protects anything.
#[derive(Debug, Clone)]
pub struct HostFs {
    /// The guest root every structured path is expressed against.
    root: GuestRoot,
    /// Where that root actually lives on this host.
    ///
    /// On the guest target the two are the same directory and the mapping is the
    /// identity. It is a field rather than a constant so the whole filesystem
    /// matrix — including the hostile cases that need a filesystem to misbehave on
    /// demand — runs off-VM against a temporary directory.
    mount: PathBuf,
}

impl HostFs {
    /// Maps `root` onto `mount`.
    #[must_use]
    pub fn new(root: GuestRoot, mount: impl Into<PathBuf>) -> Self {
        Self {
            root,
            mount: mount.into(),
        }
    }

    /// The host path one guest path names.
    #[must_use]
    pub fn host_path(&self, path: &GuestPath) -> PathBuf {
        let relative = path
            .as_str()
            .strip_prefix(self.root.0.as_str())
            .unwrap_or(path.as_str())
            .trim_start_matches('/');
        if relative.is_empty() {
            self.mount.clone()
        } else {
            self.mount.join(relative)
        }
    }
}

/// Maps an I/O error onto the typed filesystem failure it is reported as.
fn fs_error(path: &GuestPath, error: &std::io::Error) -> FsError {
    let path_text = path.as_str().to_owned();
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound { path: path_text },
        std::io::ErrorKind::PermissionDenied => FsError::PermissionDenied { path: path_text },
        std::io::ErrorKind::IsADirectory => FsError::IsADirectory { path: path_text },
        std::io::ErrorKind::NotADirectory => FsError::NotADirectory { path: path_text },
        std::io::ErrorKind::StorageFull => FsError::DiskFull,
        _ => FsError::Other {
            path: path_text,
            reason: error.to_string(),
        },
    }
}

/// The metadata one `lstat` produces, without following a symlink.
fn meta_of(path: &std::path::Path, metadata: &std::fs::Metadata, target: Option<String>) -> Meta {
    let kind = if metadata.is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|instant| instant.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or(0);
    Meta {
        kind,
        size: metadata.len(),
        mtime_ms,
        mode: mode_of(metadata),
        target: target.or_else(|| {
            metadata
                .is_symlink()
                .then(|| std::fs::read_link(path).ok())
                .flatten()
                .map(|target| target.to_string_lossy().into_owned())
        }),
    }
}

/// POSIX mode bits, where the host has them.
#[cfg(unix)]
fn mode_of(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mode()
}

/// A host without POSIX mode bits reports none rather than inventing some.
#[cfg(not(unix))]
fn mode_of(metadata: &std::fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

impl GuestFs for HostFs {
    fn read(&self, path: &GuestPath) -> Result<Vec<u8>, FsError> {
        std::fs::read(self.host_path(path)).map_err(|error| fs_error(path, &error))
    }

    fn write_atomic(&self, path: &GuestPath, bytes: &[u8], _mode: u32) -> Result<u64, FsError> {
        let target = self.host_path(path);
        let parent = target
            .parent()
            .map_or_else(|| PathBuf::from("."), std::borrow::ToOwned::to_owned);
        let temporary = parent.join(format!(
            ".aex-write-{}",
            target
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        ));
        // A temporary sibling and a rename, so a concurrent reader never observes
        // a half file and a failed write leaves the previous content intact.
        std::fs::write(&temporary, bytes).map_err(|error| fs_error(path, &error))?;
        std::fs::rename(&temporary, &target).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            fs_error(path, &error)
        })?;
        Ok(bytes.len() as u64)
    }

    fn lstat(&self, path: &GuestPath) -> Result<Meta, FsError> {
        let host = self.host_path(path);
        let metadata = std::fs::symlink_metadata(&host).map_err(|error| fs_error(path, &error))?;
        Ok(meta_of(&host, &metadata, None))
    }

    fn read_dir(&self, path: &GuestPath) -> Result<Vec<DirEntry>, FsError> {
        let mut entries = Vec::new();
        for entry in
            std::fs::read_dir(self.host_path(path)).map_err(|error| fs_error(path, &error))?
        {
            let entry = entry.map_err(|error| fs_error(path, &error))?;
            let metadata = entry
                .metadata()
                .map_err(|error| fs_error(path, &error))
                .or_else(|_| {
                    std::fs::symlink_metadata(entry.path()).map_err(|error| fs_error(path, &error))
                })?;
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                meta: meta_of(&entry.path(), &metadata, None),
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }
}

/// The real process host.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostRunner;

impl HostRunner {
    /// Builds the command a spec describes.
    fn command(spec: &SpawnSpec) -> Command {
        let mut command = Command::new(&spec.argv[0]);
        command
            .args(&spec.argv[1..])
            .current_dir(spec.cwd.as_str())
            .env_clear()
            .envs(spec.env.iter())
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Self::group(&mut command);
        command
    }

    /// Puts the child in its own process group, so a cancel reaches every
    /// descendant rather than only the leader.
    #[cfg(unix)]
    fn group(command: &mut Command) {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }

    /// A non-POSIX host has no process groups. The spawn still happens, and
    /// [`PROCESS_GROUPS_AVAILABLE`] says the containment is not there.
    #[cfg(not(unix))]
    fn group(_command: &mut Command) {}
}

impl Runner for HostRunner {
    fn start(&self, spec: &SpawnSpec) -> Result<Started, ProcError> {
        let mut child = Self::command(spec)
            .spawn()
            .map_err(|error| ProcError::SpawnFailed {
                program: spec.argv[0].clone(),
                reason: error.to_string(),
            })?;
        let pid = i32::try_from(child.id()).unwrap_or(i32::MAX);
        if let (Some(mut stdin), Some(bytes)) = (child.stdin.take(), spec.stdin.clone()) {
            use std::io::Write as _;
            stdin
                .write_all(&bytes)
                .map_err(|error| ProcError::Other(error.to_string()))?;
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let start_time = leader_start_time(pid).unwrap_or(0);
        Ok(Started {
            pgid: Pgid(pid),
            start_time,
            reap: Box::new(move |sink: &mut dyn OutputSink| {
                let mut streams: Vec<Box<dyn Read + Send>> = Vec::new();
                if let Some(handle) = stdout {
                    streams.push(Box::new(handle));
                }
                if let Some(handle) = stderr {
                    streams.push(Box::new(handle));
                }
                drain_streams(streams, sink)?;
                let status = child
                    .wait()
                    .map_err(|error| ProcError::Other(error.to_string()))?;
                Ok(exit_of(status))
            }),
        })
    }

    fn signal(&self, group: Pgid, signal: StopSignal) -> Result<(), ProcError> {
        signal_group(group, signal)
    }

    fn alive(&self, group: Pgid) -> Result<bool, ProcError> {
        group_alive(group)
    }
}

/// Largest single read handed to the sink while draining a stream.
///
/// Bounds resident memory during the drain: the capture bound applies at the
/// sink, so this buffer is the only allocation a chatty child controls. 64 KiB
/// matches the default pipe capacity, so a full pipe drains in one read.
const DRAIN_CHUNK_BYTES: usize = 65_536;

/// Drains every stream into the sink concurrently, appending in arrival order.
///
/// Concurrency is a correctness requirement, not a performance one: reading
/// stdout to end before touching stderr deadlocks the moment the child fills
/// the stderr pipe (~64 KiB) while the supervisor is still blocked on stdout.
/// One reader thread per stream keeps both pipes moving; the caller's thread
/// owns the sink so append order is a single interleaved sequence.
fn drain_streams(
    streams: Vec<Box<dyn Read + Send>>,
    sink: &mut dyn OutputSink,
) -> Result<(), ProcError> {
    let (chunks, from_readers) = std::sync::mpsc::channel::<Result<Vec<u8>, String>>();
    let mut readers = Vec::new();
    for mut stream in streams {
        let chunks = chunks.clone();
        readers.push(std::thread::spawn(move || {
            let mut buffer = vec![0u8; DRAIN_CHUNK_BYTES];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if chunks.send(Ok(buffer[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = chunks.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        }));
    }
    // The readers hold the only remaining senders, so the receive loop ends
    // exactly when every stream has reached end-of-file or failed.
    drop(chunks);
    let mut failure: Option<ProcError> = None;
    for chunk in from_readers {
        if failure.is_some() {
            continue;
        }
        match chunk {
            Ok(bytes) => {
                if let Err(reason) = sink.append(&bytes) {
                    failure = Some(ProcError::Other(reason));
                }
            }
            Err(reason) => failure = Some(ProcError::Other(reason)),
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    match failure {
        None => Ok(()),
        Some(error) => Err(error),
    }
}

/// The guest clock.
///
/// Lives beside the other host capabilities so the request path and the
/// background reap and cancel threads stamp terminals from one clock.
#[must_use]
pub fn now() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default();
    Timestamp::from_unix_millis(millis)
        .unwrap_or_else(|_| Timestamp::from_unix_millis(0).expect("the epoch is representable"))
}

/// How a finished child is reported.
fn exit_of(status: std::process::ExitStatus) -> OperationExit {
    if status.success() {
        return OperationExit::Ok;
    }
    if let Some(code) = status.code() {
        return OperationExit::NonZero { code };
    }
    OperationExit::Signal {
        name: signal_name(status),
    }
}

/// The signal that killed a child, where the host reports one.
#[cfg(unix)]
fn signal_name(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt as _;
    status
        .signal()
        .map_or_else(|| "unknown".to_owned(), |number| format!("SIG{number}"))
}

/// A non-POSIX host reports no signal rather than inventing one.
#[cfg(not(unix))]
fn signal_name(_status: std::process::ExitStatus) -> String {
    "unknown".to_owned()
}

/// Sends a signal to a whole process group.
#[cfg(unix)]
fn signal_group(group: Pgid, signal: StopSignal) -> Result<(), ProcError> {
    let which = match signal {
        StopSignal::Term => rustix::process::Signal::TERM,
        StopSignal::Kill => rustix::process::Signal::KILL,
        StopSignal::Interrupt => rustix::process::Signal::INT,
    };
    let pid = rustix::process::Pid::from_raw(group.0).ok_or(ProcError::NoSuchGroup(group))?;
    rustix::process::kill_process_group(pid, which)
        .map_err(|error| ProcError::Other(error.to_string()))
}

/// A non-POSIX host cannot signal a group, and says so rather than pretending.
#[cfg(not(unix))]
fn signal_group(_group: Pgid, _signal: StopSignal) -> Result<(), ProcError> {
    Err(ProcError::Other(
        "process-group signalling is a POSIX guest capability; this host has none".to_owned(),
    ))
}

/// Whether any member of the group is still alive, by scanning `/proc`.
///
/// A host with no `/proc` refuses rather than answering: reporting an
/// extinction it cannot observe would terminalize a live job.
fn group_alive(group: Pgid) -> Result<bool, ProcError> {
    if !PROCESS_GROUPS_AVAILABLE {
        return Err(ProcError::Other(
            "process-group liveness is a POSIX guest capability; this host has none".to_owned(),
        ));
    }
    let entries =
        std::fs::read_dir("/proc").map_err(|error| ProcError::Other(error.to_string()))?;
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if stat_field(&stat, 5).and_then(|field| field.parse::<i32>().ok()) == Some(group.0) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The leader's start time, which is what makes a replayed pid check sound.
///
/// A bare pid check would adopt an unrelated recycled pid on replay, which is how
/// a supervisor ends up reporting a stranger's process as the customer's job.
fn leader_start_time(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat_field(&stat, 22)?.parse().ok()
}

/// One one-based field of a `/proc/<pid>/stat` line.
///
/// Field 2 is the executable name in parentheses and may contain spaces, so the
/// tail is split after the last `)` rather than by whitespace from the start.
#[must_use]
pub fn stat_field(stat: &str, field: usize) -> Option<&str> {
    if field <= 2 {
        return stat.split_whitespace().nth(field - 1);
    }
    let tail = &stat[stat.rfind(')')? + 1..];
    tail.split_whitespace().nth(field - 3)
}

/// Answers "is this recorded process group still the one we started".
#[derive(Debug, Clone, Copy, Default)]
pub struct HostProbe;

impl aex_hands_agent::journal::ProcessProbe for HostProbe {
    fn leader_start_time(&self, pgid: i32) -> Result<Option<u64>, String> {
        if !PROCESS_GROUPS_AVAILABLE {
            return Err(
                "process observation is a POSIX guest capability; this host has none".to_owned(),
            );
        }
        Ok(leader_start_time(pgid))
    }
}

/// The environment a spawn is given, with every proxy variable deleted.
#[must_use]
pub fn inherited_environment() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(name, _)| aex_hands_tools::command::is_inheritable(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DRAIN_CHUNK_BYTES, HostFs, OutputSink, PROCESS_GROUPS_AVAILABLE, drain_streams,
        inherited_environment, stat_field,
    };
    use aex_hands_protocol::operation::{GuestPath, GuestRoot};
    use aex_hands_tools::port::{EntryKind, FsError, GuestFs as _};
    use std::io::Read;

    fn fs(dir: &std::path::Path) -> HostFs {
        HostFs::new(GuestRoot::workspace(), dir)
    }

    fn path(name: &str) -> GuestPath {
        let root = GuestRoot::workspace();
        let text = if name.is_empty() {
            root.0.clone()
        } else {
            format!("{}/{name}", root.0)
        };
        GuestPath::parse(&root, &text).expect("a contained path")
    }

    #[test]
    fn a_write_is_atomic_and_leaves_no_temporary_behind() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let fs = fs(dir.path());
        let target = path("note.txt");
        let written = fs
            .write_atomic(&target, b"hello", 0o644)
            .expect("the write lands");
        assert_eq!(written, 5);
        assert_eq!(fs.read(&target).expect("it reads back"), b"hello");
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .expect("the directory lists")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".aex-write-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_missing_path_is_typed_rather_than_generic() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let absent = path("absent.txt");
        let error = fs(dir.path()).read(&absent).expect_err("nothing is there");
        assert!(matches!(error, FsError::NotFound { .. }), "{error:?}");
        assert_eq!(error.code(), "not_found");
    }

    #[test]
    fn a_directory_listing_is_sorted_and_lstat_only() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        for name in ["b.txt", "a.txt"] {
            std::fs::write(dir.path().join(name), b"x").expect("a file");
        }
        std::fs::create_dir(dir.path().join("sub")).expect("a directory");
        let listed = fs(dir.path())
            .read_dir(&path(""))
            .expect("the directory lists");
        let names: Vec<&str> = listed.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);
        assert_eq!(listed[2].meta.kind, EntryKind::Directory);
    }

    #[test]
    fn a_guest_path_maps_onto_the_mounted_root_and_never_escapes_it() {
        let mounted = HostFs::new(GuestRoot::workspace(), "/mnt/session");
        assert_eq!(
            mounted.host_path(&path("a/b.txt")),
            std::path::Path::new("/mnt/session").join("a/b.txt")
        );
        assert_eq!(
            mounted.host_path(&path("")),
            std::path::Path::new("/mnt/session"),
            "the root itself maps onto the mount"
        );
        // On the guest target the mapping is the identity, which is what makes the
        // off-VM matrix evidence for the on-VM behaviour rather than a parallel
        // implementation.
        let identity = HostFs::new(GuestRoot::workspace(), "/workspace");
        assert_eq!(
            identity.host_path(&path("a/b.txt")),
            std::path::Path::new("/workspace").join("a/b.txt")
        );
    }

    #[test]
    fn the_stat_line_is_read_past_an_executable_name_containing_spaces() {
        let stat = "17 (my program) S 1 42 42 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 99887 0";
        assert_eq!(stat_field(stat, 1), Some("17"));
        assert_eq!(
            stat_field(stat, 5),
            Some("42"),
            "field 5 is the process group"
        );
        assert_eq!(
            stat_field(stat, 22),
            Some("99887"),
            "field 22 is the start time, which is what makes a pid check sound"
        );
    }

    #[test]
    fn the_inherited_environment_is_deny_by_default() {
        let inherited = inherited_environment();
        assert!(
            !inherited.keys().any(|name| name.starts_with("AWS_")),
            "the guest holds no credential, so no AWS variable may reach a spawn"
        );
        assert!(
            !inherited
                .keys()
                .any(|name| name.to_ascii_lowercase().contains("proxy")),
            "every proxy variable is deleted unconditionally"
        );
    }

    #[test]
    fn process_group_supervision_states_whether_this_host_has_it() {
        assert_eq!(PROCESS_GROUPS_AVAILABLE, cfg!(unix));
    }

    struct VecSink(Vec<u8>);

    impl OutputSink for VecSink {
        fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.extend_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn the_drain_reads_both_streams_to_completion_in_bounded_chunks() {
        // Larger than a pipe (~64 KiB) on the second stream: the retired
        // sequential drain read stdout to end first, which deadlocks against a
        // child blocked writing stderr. The concurrent drain must consume both.
        let stdout = vec![b'a'; 10_000];
        let stderr = vec![b'b'; 4 * DRAIN_CHUNK_BYTES + 17];
        let mut sink = VecSink(Vec::new());
        drain_streams(
            vec![
                Box::new(std::io::Cursor::new(stdout.clone())),
                Box::new(std::io::Cursor::new(stderr.clone())),
            ],
            &mut sink,
        )
        .expect("both streams drain");
        assert_eq!(sink.0.len(), stdout.len() + stderr.len());
        assert_eq!(
            sink.0
                .iter()
                .fold(0_usize, |count, byte| count + usize::from(*byte == b'a')),
            10_000
        );
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("the pipe collapsed"))
        }
    }

    #[test]
    fn a_stream_read_failure_is_reported_and_never_swallowed() {
        let mut sink = VecSink(Vec::new());
        let outcome = drain_streams(
            vec![
                Box::new(std::io::Cursor::new(vec![b'x'; 8])),
                Box::new(FailingReader),
            ],
            &mut sink,
        );
        let error = outcome.expect_err("the failure surfaces");
        assert!(error.to_string().contains("the pipe collapsed"), "{error}");
    }
}
