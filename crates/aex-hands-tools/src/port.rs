//! The ports every executor runs over.
//!
//! Executors are pure functions of an injected filesystem and process host, so the
//! whole matrix — including the hostile cases that need a filesystem to misbehave
//! on demand — runs off-VM.
//!
//! [`GuestPath`] containment is a **structural tool contract, not a security
//! boundary**. H-BOUNDARY grants the customer real root, and `shell_exec` reaches
//! the entire filesystem by design. Structured file tools stay inside the workspace
//! root so revision checking and persist have meaning, not because staying inside
//! it protects anything.

use aex_hands_protocol::operation::GuestPath;
use aex_wire::ids::ContentHash;

/// What kind of thing a path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link. Never followed by a structured tool.
    Symlink,
    /// Anything else: a socket, a fifo, a device.
    Other,
}

impl EntryKind {
    /// The single-word rendering the list and stat tools emit.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "dir",
            Self::Symlink => "link",
            Self::Other => "other",
        }
    }
}

/// What `lstat` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// What kind of thing it is.
    pub kind: EntryKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in epoch milliseconds.
    pub mtime_ms: i64,
    /// POSIX mode bits.
    pub mode: u32,
    /// The link target, when the entry is a symlink.
    pub target: Option<String>,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// The entry's own name, not a path.
    pub name: String,
    /// Its metadata, from `lstat`.
    pub meta: Meta,
}

/// Why a filesystem call failed.
///
/// Every variant maps onto exactly one `OperationFailureCode`, so a guest failure
/// is never reported as a generic error the model has to guess at.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    /// Nothing exists at the path.
    #[error("`{path}` does not exist")]
    NotFound {
        /// Which path.
        path: String,
    },
    /// The path names a directory where a file was required.
    #[error("`{path}` is a directory")]
    IsADirectory {
        /// Which path.
        path: String,
    },
    /// The path names a file where a directory was required.
    #[error("`{path}` is not a directory")]
    NotADirectory {
        /// Which path.
        path: String,
    },
    /// The kernel refused.
    #[error("permission denied on `{path}`")]
    PermissionDenied {
        /// Which path.
        path: String,
    },
    /// The filesystem is full.
    #[error("the guest disk is full")]
    DiskFull,
    /// Anything else the host reported.
    #[error("filesystem error on `{path}`: {reason}")]
    Other {
        /// Which path.
        path: String,
        /// Why.
        reason: String,
    },
}

impl FsError {
    /// The stable failure code this error is reported as.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::IsADirectory { .. } => "is_a_directory",
            Self::NotADirectory { .. } => "not_a_file",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::DiskFull => "disk_full",
            Self::Other { .. } => "invalid_argument",
        }
    }
}

/// The guest filesystem.
pub trait GuestFs: Send + Sync {
    /// Reads a whole file.
    ///
    /// # Errors
    ///
    /// See [`FsError`].
    fn read(&self, path: &GuestPath) -> Result<Vec<u8>, FsError>;

    /// Writes a file atomically: a temporary sibling, then a rename.
    ///
    /// Atomicity matters because a concurrent reader must never observe a half
    /// file, and because a failed write must leave the previous content intact.
    ///
    /// # Errors
    ///
    /// See [`FsError`].
    fn write_atomic(&self, path: &GuestPath, bytes: &[u8], mode: u32) -> Result<u64, FsError>;

    /// Stats a path **without** following a symlink.
    ///
    /// # Errors
    ///
    /// See [`FsError`].
    fn lstat(&self, path: &GuestPath) -> Result<Meta, FsError>;

    /// Lists a directory's immediate children, sorted by name.
    ///
    /// # Errors
    ///
    /// See [`FsError`].
    fn read_dir(&self, path: &GuestPath) -> Result<Vec<DirEntry>, FsError>;
}

/// The `blake3` digest of some bytes, which is what a revision check compares.
#[must_use]
pub fn digest(bytes: &[u8]) -> ContentHash {
    ContentHash::from_bytes(*blake3::hash(bytes).as_bytes())
}

/// A process group identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pgid(pub i32);

/// Why a process call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProcError {
    /// The program could not be started.
    #[error("cannot spawn `{program}`: {reason}")]
    SpawnFailed {
        /// The program that was asked for.
        program: String,
        /// Why.
        reason: String,
    },
    /// The named interpreter is not installed.
    #[error("`{interpreter}` is not installed in this image")]
    InterpreterUnavailable {
        /// Which interpreter.
        interpreter: String,
    },
    /// No such process group.
    #[error("no process group {0:?}")]
    NoSuchGroup(Pgid),
    /// Anything else the host reported.
    #[error("process error: {0}")]
    Other(String),
}

impl ProcError {
    /// The stable failure code this error is reported as.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SpawnFailed { .. } => "spawn_failed",
            Self::InterpreterUnavailable { .. } => "interpreter_unavailable",
            Self::NoSuchGroup(_) => "not_found",
            Self::Other(_) => "invalid_argument",
        }
    }
}

/// The guest process host.
pub trait GuestProc: Send + Sync {
    /// Starts a command in its own process group.
    ///
    /// # Errors
    ///
    /// See [`ProcError`].
    fn spawn_group(&self, spec: &crate::command::SpawnSpec) -> Result<Pgid, ProcError>;

    /// Signals a whole process group.
    ///
    /// # Errors
    ///
    /// See [`ProcError`].
    fn signal_group(
        &self,
        group: Pgid,
        signal: aex_hands_protocol::operation::StopSignal,
    ) -> Result<(), ProcError>;

    /// Whether any member of the group is still alive.
    ///
    /// # Errors
    ///
    /// See [`ProcError`].
    fn group_alive(&self, group: Pgid) -> Result<bool, ProcError>;
}
