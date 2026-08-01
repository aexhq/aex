//! What Brain asks the guest to do, and what came back.
//!
//! [`GuestPath`] is the security-critical type here: it is parsed against an
//! explicit root and rejects traversal, embedded NUL, non-UTF-8 and relative
//! form. It never *normalizes* a path, because normalizing means two different
//! requests become one identity, and identity is what the call hash is over.

use aex_wire::ids::ContentHash;
use aex_wire::types::{DecimalU128, Timestamp};
use serde::{Deserialize, Serialize};

/// The guest filesystem root every path is resolved against.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestRoot(pub String);

impl GuestRoot {
    /// The default workspace root.
    #[must_use]
    pub fn workspace() -> Self {
        Self("/workspace".to_owned())
    }
}

/// Why a guest path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GuestPathError {
    /// The path was empty or longer than the bound.
    #[error("a guest path is 1..=4096 bytes")]
    Length,
    /// The path was not absolute.
    #[error("a guest path must be absolute")]
    Relative,
    /// The path contained a NUL or another control byte.
    #[error("a guest path may not contain a control byte")]
    ControlByte,
    /// The path contained a `.` or `..` segment.
    #[error("a guest path may not contain a `.` or `..` segment")]
    Traversal,
    /// The path resolved outside the declared root.
    #[error("a guest path must stay inside its root")]
    OutsideRoot,
}

/// A path inside the guest, validated against an explicit root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestPath(String);

impl GuestPath {
    /// Largest accepted byte length.
    pub const MAX_BYTES: usize = 4096;

    /// Parses a path against `root`.
    ///
    /// # Errors
    ///
    /// Returns [`GuestPathError`] for an empty or oversized path, a relative
    /// path, a control byte, a `.` or `..` segment, or a path outside `root`.
    pub fn parse(root: &GuestRoot, text: &str) -> Result<Self, GuestPathError> {
        if text.is_empty() || text.len() > Self::MAX_BYTES {
            return Err(GuestPathError::Length);
        }
        if text.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
            return Err(GuestPathError::ControlByte);
        }
        if !text.starts_with('/') {
            return Err(GuestPathError::Relative);
        }
        for segment in text[1..].split('/') {
            if segment == "." || segment == ".." {
                return Err(GuestPathError::Traversal);
            }
        }
        let root_path = root.0.trim_end_matches('/');
        let inside =
            text == root_path || text.starts_with(&format!("{root_path}/")) || root_path.is_empty();
        if !inside {
            return Err(GuestPathError::OutsideRoot);
        }
        Ok(Self(text.to_owned()))
    }

    /// The path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An environment variable name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvName(pub String);

/// An environment variable value. Never rendered in a diagnostic.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvValue(String);

impl EnvValue {
    /// Wraps a value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that has to set it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for EnvValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EnvValue(<redacted>)")
    }
}

/// A POSIX mode for a written file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    /// `0644`.
    ReadWrite,
    /// `0755`.
    Executable,
}

/// The signal a process stop request sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSignal {
    /// Ask politely.
    Term,
    /// Insist.
    Kill,
    /// Interrupt, as a terminal would.
    Interrupt,
}

/// A guest process identity minted by the guest agent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestProcessId(pub String);

/// An inclusive byte range over a guest file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ByteRangeRequest {
    /// First byte, inclusive.
    pub start: DecimalU128,
    /// Last byte, inclusive.
    pub end_inclusive: DecimalU128,
}

/// What a search actually matches.
///
/// Enumerated rather than "a regex string": an arbitrary pattern language is an
/// unbounded input to a matcher running against customer-controlled data, and
/// the tools stream asked for this to be a closed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "pattern",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SearchPattern {
    /// A literal substring.
    Literal {
        /// The needle.
        needle: String,
        /// Whether case matters.
        case_sensitive: bool,
    },
    /// A glob over the path, not the contents.
    Glob {
        /// The glob.
        glob: String,
    },
    /// A bounded regular expression, rejected if it exceeds the size bound.
    Regex {
        /// The expression.
        expression: String,
        /// Whether case matters.
        case_sensitive: bool,
    },
}

impl SearchPattern {
    /// Largest accepted pattern length.
    pub const MAX_BYTES: usize = 1024;

    /// Whether the pattern is inside its size bound.
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        let length = match self {
            Self::Literal { needle, .. } => needle.len(),
            Self::Glob { glob } => glob.len(),
            Self::Regex { expression, .. } => expression.len(),
        };
        length > 0 && length <= Self::MAX_BYTES
    }
}

/// One replacement inside a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PatchHunk {
    /// The exact text that must currently be there.
    pub expected: String,
    /// What replaces it.
    pub replacement: String,
    /// How many occurrences to replace; zero means every occurrence.
    pub occurrences: u32,
}

/// An edit, expressed as exact replacements rather than a diff format.
///
/// A unified diff would need a parser on the hostile side of the boundary and
/// would make "did this apply cleanly" a fuzzy question. Exact expected text
/// makes it a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Patch {
    /// The replacements, applied in order.
    pub hunks: Vec<PatchHunk>,
}

/// A reference to content Brain has already stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContentRef {
    /// The content digest.
    pub digest: ContentHash,
    /// How many bytes it is.
    pub bytes: DecimalU128,
}

/// What Brain asked the guest to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OperationRequest {
    /// Run a program.
    Exec {
        /// Argument vector; there is no shell.
        argv: Vec<String>,
        /// Working directory.
        cwd: GuestPath,
        /// Environment additions.
        env: Vec<(EnvName, EnvValue)>,
        /// Standard input, when there is any.
        stdin: Option<ContentRef>,
    },
    /// Read a file.
    ReadFile {
        /// Which file.
        path: GuestPath,
        /// A bounded range, when only part is wanted.
        range: Option<ByteRangeRequest>,
    },
    /// Write a file.
    WriteFile {
        /// Which file.
        path: GuestPath,
        /// The POSIX mode.
        mode: FileMode,
        /// The content.
        content: ContentRef,
    },
    /// Edit a file, guarded by its current digest.
    EditFile {
        /// Which file.
        path: GuestPath,
        /// The digest the file must currently have.
        expected: ContentHash,
        /// The replacements.
        patch: Patch,
    },
    /// List a directory.
    ListDir {
        /// Which directory.
        path: GuestPath,
        /// Whether to descend.
        recursive: bool,
        /// How many entries at most.
        limit: u32,
    },
    /// Stat a path.
    StatPath {
        /// Which path.
        path: GuestPath,
    },
    /// Search a subtree.
    Search {
        /// Where to search.
        root: GuestPath,
        /// What to match.
        pattern: SearchPattern,
        /// How many matches at most.
        limit: u32,
    },
    /// Read the status of a background process.
    ProcessStatus {
        /// Which process.
        process: GuestProcessId,
    },
    /// Stop a background process.
    ProcessStop {
        /// Which process.
        process: GuestProcessId,
        /// Which signal.
        signal: StopSignal,
    },
    /// Materialize a stored tree into the workspace.
    Materialize {
        /// The tree root digest.
        root: ContentHash,
    },
    /// Persist the workspace.
    Persist {
        /// Paths to include.
        include: Vec<GuestPath>,
        /// Paths to exclude.
        exclude: Vec<GuestPath>,
    },
    /// Run a registered custom tool.
    RegisteredTool {
        /// The tool name.
        name: aex_wire::ids::ResourceName,
        /// The manifest digest the arguments were validated against.
        manifest: ContentHash,
        /// The canonical arguments.
        args: aex_wire::CanonicalJson,
    },
}

/// Bounds the guest cannot raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationBounds {
    /// Largest terminal body Brain will accept.
    pub max_output_bytes: u64,
    /// Largest single frame, checked before any allocation.
    pub max_frame_bytes: u32,
    /// Wall-clock ceiling.
    pub max_wall_ms: u64,
    /// How many operations may be in flight at once.
    pub max_concurrent_operations: u16,
}

/// How an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    /// It completed.
    Succeeded,
    /// It failed.
    Failed,
    /// It was cancelled.
    Cancelled,
    /// It lost its substrate mid-flight.
    Interrupted,
}

/// The exit condition of an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "exit",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OperationExit {
    /// Clean exit.
    Ok,
    /// A non-zero exit status.
    NonZero {
        /// The status.
        code: i32,
    },
    /// Killed by a signal.
    Signal {
        /// The signal name.
        name: String,
    },
    /// It exceeded its wall bound.
    Timeout,
}

/// Why an operation could not run or did not finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OperationFailure {
    /// A stable, non-localized reason.
    pub reason: String,
    /// A bounded diagnostic; never a guest-controlled payload.
    pub detail: Option<String>,
    /// Whether an identical retry can succeed.
    pub retryable: bool,
}

/// What Brain records when an operation reaches a terminal state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalMetadata {
    /// How it ended.
    pub state: TerminalState,
    /// The exit condition.
    pub exit: OperationExit,
    /// When it started.
    pub started_at: Timestamp,
    /// When it ended.
    pub ended_at: Timestamp,
    /// How many bytes the terminal body is.
    pub body_len: u64,
    /// The digest of the terminal body.
    pub digest: ContentHash,
    /// Whether the body was cut short by the output bound.
    pub truncated: bool,
    /// Why it failed, when it did.
    pub failure: Option<OperationFailure>,
}

/// How a terminal body is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    /// Brain holds the connection and receives diagnostics as they happen.
    Attached,
    /// Brain polls and pulls the body afterwards.
    Detached,
}
