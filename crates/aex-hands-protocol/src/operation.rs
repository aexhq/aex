//! What Brain asks the guest to do, and what came back.
//!
//! [`GuestPath`] is the security-critical type here: it is parsed against an
//! explicit root and rejects traversal, embedded NUL, non-UTF-8 and relative
//! form. It never *normalizes* a path, because normalizing means two different
//! requests become one identity, and identity is what the call hash is over.

use aex_wire::ids::ContentHash;
use aex_wire::types::{DecimalU128, HttpsUrl, Timestamp};
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

/// Why a content endpoint was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContentEndpointError {
    /// The scheme was not `https`.
    #[error("a content endpoint is an `https://host[:port]` origin")]
    NotAnHttpsOrigin,
    /// The value carried more than an origin.
    #[error("a content endpoint carries no path, query, fragment or user information")]
    NotBareOrigin,
    /// The authority was empty or oversized.
    #[error("a content endpoint authority is 1..=255 bytes")]
    Host,
}

/// The one origin a guest may move content through.
///
/// Pinned into the guest at launch and deliberately **not** carried by any
/// request. A plan that named its own trust anchor would authorize whichever
/// host it chose, which is exactly the hole a credential-free guest cannot
/// otherwise close: it has nothing else to check a URL against.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentEndpoint(String);

impl ContentEndpoint {
    /// Largest accepted authority length.
    pub const MAX_AUTHORITY_BYTES: usize = 255;

    /// Parses a bare `https://host[:port]` origin, lowercasing the authority.
    ///
    /// # Errors
    ///
    /// Returns [`ContentEndpointError`] for a non-`https` scheme, an empty or
    /// oversized authority, or anything beyond the origin.
    pub fn parse(text: &str) -> Result<Self, ContentEndpointError> {
        let rest = text
            .strip_prefix("https://")
            .ok_or(ContentEndpointError::NotAnHttpsOrigin)?;
        if rest.is_empty() || rest.len() > Self::MAX_AUTHORITY_BYTES {
            return Err(ContentEndpointError::Host);
        }
        if rest.contains(['/', '?', '#', '@']) {
            return Err(ContentEndpointError::NotBareOrigin);
        }
        Ok(Self(format!("https://{}", rest.to_ascii_lowercase())))
    }

    /// The normalized origin.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a presigned grant lets the guest do with one object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    /// A `GET` of content Brain already holds.
    Fetch,
    /// A conditional-create `PUT` to a content-addressed key.
    Store,
}

/// Why a presigned plan could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanRejection {
    /// The plan named a host the guest was not launched against.
    #[error("plan names origin `{found}`, the guest is pinned to `{expected}`")]
    ForeignOrigin {
        /// The pinned origin.
        expected: String,
        /// The origin the plan named.
        found: String,
    },
    /// The grant window has closed.
    #[error("plan expired at {expires_at} and it is now {now}")]
    Expired {
        /// When the grant stopped working.
        expires_at: Timestamp,
        /// The instant the caller presented.
        now: Timestamp,
    },
    /// The caller is about to do something the grant does not cover.
    #[error("plan is a {found:?} grant, the caller is performing a {expected:?}")]
    WrongDirection {
        /// What the caller is doing.
        expected: TransferDirection,
        /// What the grant covers.
        found: TransferDirection,
    },
    /// A fetch grant with nothing to verify the bytes against.
    #[error("a fetch plan must declare the digest the fetched bytes must have")]
    MissingDigest,
    /// A store grant claiming a digest it cannot enforce.
    #[error("a store plan declares a digest it cannot enforce")]
    UnexpectedDigest,
    /// The declared ceiling is zero or above the published bound.
    #[error("plan covers {max_bytes} bytes, which is outside 1..={limit}")]
    Unbounded {
        /// What the plan declared.
        max_bytes: u64,
        /// The published bound.
        limit: u64,
    },
}

/// A Brain-issued capability over exactly one object.
///
/// The guest holds no AWS credential (H-BOUNDARY), so this is the only way it
/// moves bytes at all. Everything that bounds the capability travels with it:
/// which direction it covers, when it stops working, how many bytes it is good
/// for, and — for a fetch — the digest the bytes must have.
///
/// The URL is private and [`PresignedPlan::authorize`] is the only way out of
/// the type. That accessor demands the [`ContentEndpoint`] pinned into the guest
/// at launch, so a forged or replayed frame cannot point the guest at an
/// arbitrary host, and it demands the current instant, so an expired grant
/// cannot be used by a caller that simply forgot to look.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PresignedPlan {
    /// The presigned URL. Private: see the type documentation.
    url: HttpsUrl,
    /// What the grant covers.
    pub direction: TransferDirection,
    /// After this instant the grant is dead.
    pub expires_at: Timestamp,
    /// How many bytes the grant is good for.
    pub max_bytes: u64,
    /// The digest a fetch must produce. Always present on a fetch, always absent
    /// on a store.
    pub expected_digest: Option<ContentHash>,
}

impl PresignedPlan {
    /// The largest plan or manifest document a grant may cover.
    ///
    /// Matches `content.bundle_expand`: 16 MiB is the ceiling on the expansion
    /// plan and on the survey manifest alike.
    pub const MAX_PLAN_BYTES: u64 = 16 * 1024 * 1024;

    /// A grant to fetch one object and verify it against `digest`.
    #[must_use]
    pub fn fetch(
        url: HttpsUrl,
        expires_at: Timestamp,
        digest: ContentHash,
        max_bytes: u64,
    ) -> Self {
        Self {
            url,
            direction: TransferDirection::Fetch,
            expires_at,
            max_bytes,
            expected_digest: Some(digest),
        }
    }

    /// A grant to store one object the guest has yet to build.
    #[must_use]
    pub fn store(url: HttpsUrl, expires_at: Timestamp, max_bytes: u64) -> Self {
        Self {
            url,
            direction: TransferDirection::Store,
            expires_at,
            max_bytes,
            expected_digest: None,
        }
    }

    /// The origin the plan names, which is safe to print.
    #[must_use]
    pub fn origin(&self) -> &str {
        let text = self.url.as_str();
        let end = text["https://".len()..]
            .find(['/', '?', '#'])
            .map_or(text.len(), |offset| offset + "https://".len());
        &text[..end]
    }

    /// The URL, if and only if every condition of the grant holds.
    ///
    /// # Errors
    ///
    /// Returns [`PlanRejection`] when the plan names a different origin from the
    /// one the guest was launched against, when the grant has expired, when the
    /// caller is performing the other direction, when a fetch pins no digest or
    /// a store pins one, or when the byte ceiling is zero or above
    /// [`PresignedPlan::MAX_PLAN_BYTES`].
    pub fn authorize(
        &self,
        endpoint: &ContentEndpoint,
        now: Timestamp,
        performing: TransferDirection,
    ) -> Result<&HttpsUrl, PlanRejection> {
        if self.direction != performing {
            return Err(PlanRejection::WrongDirection {
                expected: performing,
                found: self.direction,
            });
        }
        let origin = self.origin().to_ascii_lowercase();
        if origin != endpoint.as_str() {
            return Err(PlanRejection::ForeignOrigin {
                expected: endpoint.as_str().to_owned(),
                found: origin,
            });
        }
        if now.unix_millis() > self.expires_at.unix_millis() {
            return Err(PlanRejection::Expired {
                expires_at: self.expires_at,
                now,
            });
        }
        match (self.direction, self.expected_digest) {
            (TransferDirection::Fetch, None) => return Err(PlanRejection::MissingDigest),
            (TransferDirection::Store, Some(_)) => return Err(PlanRejection::UnexpectedDigest),
            _ => {}
        }
        if self.max_bytes == 0 || self.max_bytes > Self::MAX_PLAN_BYTES {
            return Err(PlanRejection::Unbounded {
                max_bytes: self.max_bytes,
                limit: Self::MAX_PLAN_BYTES,
            });
        }
        Ok(&self.url)
    }
}

impl std::fmt::Debug for PresignedPlan {
    /// Names the origin and the bounds; never the signature.
    ///
    /// A presigned URL is bearer material for one object, so anything that logs
    /// a request must not thereby log the capability.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PresignedPlan")
            .field("origin", &self.origin())
            .field("direction", &self.direction)
            .field("expires_at", &self.expires_at)
            .field("max_bytes", &self.max_bytes)
            .field("expected_digest", &self.expected_digest)
            .finish_non_exhaustive()
    }
}

/// The viewport a browser session opens with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BrowserViewport {
    /// Width in CSS pixels.
    pub width: u32,
    /// Height in CSS pixels.
    pub height: u32,
}

/// One instruction to the headless browser.
///
/// `Evaluate` is present deliberately: the customer is root inside the guest and
/// can attach to the same debugging port regardless, so forbidding it would be
/// theatre rather than a boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "command",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum BrowserCommand {
    /// Start a session on a page. The only command that mints a session.
    Open {
        /// Where to start.
        url: HttpsUrl,
        /// The viewport, when the caller wants one other than the default.
        viewport: Option<BrowserViewport>,
        /// How long the open may take.
        timeout_ms: u32,
    },
    /// Go to another page in the session.
    Navigate {
        /// Where to go.
        url: HttpsUrl,
    },
    /// Click one element.
    Click {
        /// Which element.
        selector: String,
    },
    /// Type into one element.
    Type {
        /// Which element.
        selector: String,
        /// What to type.
        text: String,
    },
    /// Send one key.
    Key {
        /// The key name.
        key: String,
    },
    /// Scroll the page or one element.
    Scroll {
        /// Which element, when not the page.
        selector: Option<String>,
        /// How far, in CSS pixels; negative scrolls up.
        delta_y: i32,
    },
    /// Wait a bounded time.
    Wait {
        /// How long.
        ms: u32,
    },
    /// Capture the viewport.
    Screenshot,
    /// Extract text from the page or one element.
    ReadText {
        /// Which element, when not the page.
        selector: Option<String>,
    },
    /// Evaluate an expression in the page.
    Evaluate {
        /// The expression.
        expression: String,
    },
    /// End the session.
    Close,
}

impl BrowserCommand {
    /// Largest accepted selector length.
    pub const MAX_SELECTOR_BYTES: usize = 1024;
    /// Largest accepted typed text length.
    pub const MAX_TEXT_BYTES: usize = 32_768;
    /// Largest accepted evaluated expression length.
    pub const MAX_EXPRESSION_BYTES: usize = 32_768;
    /// Largest accepted key name length.
    pub const MAX_KEY_BYTES: usize = 64;
    /// Largest accepted per-command wait.
    pub const MAX_WAIT_MS: u32 = 30_000;
    /// Accepted open timeout window.
    pub const OPEN_TIMEOUT_MS: std::ops::RangeInclusive<u32> = 1_000..=120_000;
    /// Accepted viewport width.
    pub const VIEWPORT_WIDTH: std::ops::RangeInclusive<u32> = 320..=3_840;
    /// Accepted viewport height.
    pub const VIEWPORT_HEIGHT: std::ops::RangeInclusive<u32> = 240..=2_160;

    /// Whether the command acts on a session that already exists.
    #[must_use]
    pub const fn needs_session(&self) -> bool {
        !matches!(self, Self::Open { .. })
    }

    /// Whether every operand is inside its published bound.
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        let selector_ok =
            |selector: &String| !selector.is_empty() && selector.len() <= Self::MAX_SELECTOR_BYTES;
        match self {
            Self::Open {
                viewport,
                timeout_ms,
                ..
            } => {
                Self::OPEN_TIMEOUT_MS.contains(timeout_ms)
                    && viewport.is_none_or(|viewport| {
                        Self::VIEWPORT_WIDTH.contains(&viewport.width)
                            && Self::VIEWPORT_HEIGHT.contains(&viewport.height)
                    })
            }
            Self::Navigate { .. } | Self::Screenshot | Self::Close => true,
            Self::Click { selector } => selector_ok(selector),
            Self::Type { selector, text } => {
                selector_ok(selector) && text.len() <= Self::MAX_TEXT_BYTES
            }
            Self::Key { key } => !key.is_empty() && key.len() <= Self::MAX_KEY_BYTES,
            Self::Scroll { selector, .. } | Self::ReadText { selector } => {
                selector.as_ref().is_none_or(selector_ok)
            }
            Self::Wait { ms } => *ms <= Self::MAX_WAIT_MS,
            Self::Evaluate { expression } => {
                !expression.is_empty() && expression.len() <= Self::MAX_EXPRESSION_BYTES
            }
        }
    }
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
    ///
    /// An observation, answered from `lstat` alone. It reports mode, mtime and
    /// size and deliberately carries **no content digest**: listing is an
    /// observation and must not turn into an implicit full-tree hash.
    ListDir {
        /// Which directory.
        path: GuestPath,
        /// Whether to descend.
        recursive: bool,
        /// How many entries at most.
        limit: u32,
        /// Resume strictly after this path.
        ///
        /// Entries come back ascending by path, so the last path of a page is
        /// the whole cursor a listing needs. Absent means start at the
        /// beginning.
        after: Option<GuestPath>,
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
    /// Read the status of a background process and a bounded window of its
    /// output.
    ///
    /// The window is explicit because a tail loses backlog: a caller that only
    /// ever sees the end of the buffer can never reconstruct what a long-running
    /// process printed while it was not looking.
    ProcessStatus {
        /// Which process.
        process: GuestProcessId,
        /// The first output byte the caller has not read.
        from_offset: u64,
        /// How many bytes to return, capped at
        /// [`OperationRequest::MAX_OUTPUT_WINDOW_BYTES`] and trimmed back to a
        /// UTF-8 boundary.
        max_bytes: u32,
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
        /// The tree root digest. Identity only: the guest holds no credential
        /// and cannot resolve a hash to bytes, so this names what is being
        /// materialized rather than where to get it.
        root: ContentHash,
        /// The fetch grant for the expansion plan, whose entries carry a
        /// presigned URL and a digest per file.
        plan: PresignedPlan,
    },
    /// Drive the headless browser.
    ///
    /// Present in the protocol whether or not the running image carries the
    /// capability: a gate that has nothing to reject can never fail closed.
    Browser {
        /// Which browser session, when the command acts on an existing one.
        /// [`BrowserCommand::Open`] mints the session and carries none; every
        /// other command names one. [`OperationRequest::browser_target_is_coherent`]
        /// is the check.
        session: Option<GuestProcessId>,
        /// What to do.
        command: BrowserCommand,
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

impl OperationRequest {
    /// The largest output window one [`OperationRequest::ProcessStatus`] may
    /// return, before the UTF-8 boundary trim.
    pub const MAX_OUTPUT_WINDOW_BYTES: u32 = 1_000_000;

    /// Whether the request needs the browser capability.
    ///
    /// Exhaustive by construction: a new arm has to say which side of the gate
    /// it is on before this compiles.
    #[must_use]
    pub const fn requires_browser(&self) -> bool {
        match self {
            Self::Browser { .. } => true,
            Self::Exec { .. }
            | Self::ReadFile { .. }
            | Self::WriteFile { .. }
            | Self::EditFile { .. }
            | Self::ListDir { .. }
            | Self::StatPath { .. }
            | Self::Search { .. }
            | Self::ProcessStatus { .. }
            | Self::ProcessStop { .. }
            | Self::Materialize { .. }
            | Self::RegisteredTool { .. } => false,
        }
    }

    /// Whether a browser request names a session exactly when its command needs
    /// one.
    ///
    /// A non-browser request is trivially coherent.
    #[must_use]
    pub const fn browser_target_is_coherent(&self) -> bool {
        match self {
            Self::Browser { session, command } => command.needs_session() == session.is_some(),
            _ => true,
        }
    }
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
