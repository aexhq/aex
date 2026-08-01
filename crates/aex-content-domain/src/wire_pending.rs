//! Minimal stand-ins for the vocabulary the contract stream owns.
//!
//! Every item here is temporary. The contracts stream is implementing
//! `aex-wire`, `aex-internal-contracts`, `aex-payment-contracts` and
//! `aex-hands-protocol` on a separate branch; until those crates carry real
//! definitions the six regional pure crates cannot compile against them. Each
//! item is therefore declared once, here, in the lowest crate of the regional
//! pure stack and re-exported by the other five, so a single `use` rewrite at
//! merge replaces the whole module.
//!
//! Nothing in this module encodes a domain rule. It is identifiers, closed
//! projection enums, the error-code vocabulary and the one canonicalization
//! entry point.

use core::fmt;

use crate::canonical::CanonicalError;

/// Crockford base32 alphabet, excluding `I`, `L`, `O` and `U`.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Number of Crockford characters in the body of a prefixed identifier.
pub const ID_BODY_LEN: usize = 26;

/// Reverse table for [`CROCKFORD`]; `0xff` marks a character outside the
/// alphabet.
const CROCKFORD_INVERSE: [u8; 256] = build_crockford_inverse();

const fn build_crockford_inverse() -> [u8; 256] {
    let mut table = [0xff_u8; 256];
    let mut i = 0;
    while i < 32 {
        table[CROCKFORD[i] as usize] = i as u8;
        i += 1;
    }
    table
}

/// Encodes 128 bits as exactly 26 Crockford base32 characters.
fn encode_crockford(value: u128) -> [u8; ID_BODY_LEN] {
    let mut out = [b'0'; ID_BODY_LEN];
    let mut remaining = value;
    let mut index = ID_BODY_LEN;
    while index > 0 {
        index -= 1;
        out[index] = CROCKFORD[(remaining & 0x1f) as usize];
        remaining >>= 5;
    }
    out
}

/// Decodes exactly 26 Crockford base32 characters into 128 bits.
///
/// Rejects any character outside the alphabet, lower case, and a leading
/// character above `7` (which would not fit in 128 bits).
fn decode_crockford(body: &[u8]) -> Result<u128, IdError> {
    if body.len() != ID_BODY_LEN {
        return Err(IdError::BodyLength { found: body.len() });
    }
    let mut value: u128 = 0;
    for (position, byte) in body.iter().enumerate() {
        let digit = CROCKFORD_INVERSE[*byte as usize];
        if digit == 0xff {
            return Err(IdError::BodyCharacter { position });
        }
        if position == 0 && digit > 7 {
            return Err(IdError::BodyOverflow);
        }
        value = (value << 5) | u128::from(digit);
    }
    Ok(value)
}

/// Why a prefixed identifier failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The `<prefix>_` part was absent or did not match the expected prefix.
    #[error("expected identifier prefix `{expected}_`")]
    Prefix {
        /// The prefix the target type requires.
        expected: &'static str,
    },
    /// The body was not exactly [`ID_BODY_LEN`] characters.
    #[error("identifier body must be {ID_BODY_LEN} characters, found {found}")]
    BodyLength {
        /// The observed body length.
        found: usize,
    },
    /// A body character was outside the Crockford base32 alphabet.
    #[error("identifier body character {position} is not Crockford base32")]
    BodyCharacter {
        /// Zero-based index of the offending character.
        position: usize,
    },
    /// The leading character encoded a value above 128 bits.
    #[error("identifier body encodes more than 128 bits")]
    BodyOverflow,
}

macro_rules! prefixed_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u128);

        impl $name {
            #[doc = concat!("Textual prefix of a `", stringify!($name), "`.")]
            pub const PREFIX: &'static str = $prefix;

            #[doc = concat!("Builds a `", stringify!($name), "` from raw 128-bit entropy.")]
            #[must_use]
            pub const fn from_u128(value: u128) -> Self {
                Self(value)
            }

            /// The raw 128-bit value.
            #[must_use]
            pub const fn to_u128(self) -> u128 {
                self.0
            }

            /// The raw value in big-endian byte order, for canonical hashing.
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 16] {
                self.0.to_be_bytes()
            }

            /// Parses the canonical `<prefix>_<26 Crockford>` form.
            ///
            /// # Errors
            ///
            /// Returns [`IdError`] when the prefix, length or alphabet is wrong.
            pub fn parse(text: &str) -> Result<Self, IdError> {
                let bytes = text.as_bytes();
                let prefix = $prefix.as_bytes();
                if bytes.len() <= prefix.len()
                    || &bytes[..prefix.len()] != prefix
                    || bytes[prefix.len()] != b'_'
                {
                    return Err(IdError::Prefix { expected: $prefix });
                }
                decode_crockford(&bytes[prefix.len() + 1..]).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let body = encode_crockford(self.0);
                f.write_str($prefix)?;
                f.write_str("_")?;
                // Every byte written by `encode_crockford` comes from the
                // ASCII-only Crockford table, so this is always valid UTF-8.
                for byte in body {
                    f.write_str(core::str::from_utf8(core::slice::from_ref(&byte)).unwrap_or("?"))?;
                }
                Ok(())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl core::str::FromStr for $name {
            type Err = IdError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::parse(text)
            }
        }
    };
}

prefixed_id!(
    /// Identifies one session.
    SessionId,
    "ses"
);
prefixed_id!(
    /// Identifies one message inside a session.
    MessageId,
    "msg"
);
prefixed_id!(
    /// Identifies one run of a session.
    RunId,
    "run"
);
prefixed_id!(
    /// Identifies one agent inside a session.
    AgentId,
    "agt"
);
prefixed_id!(
    /// Identifies one tool call.
    ToolCallId,
    "tcl"
);
prefixed_id!(
    /// Identifies one durable operation.
    OperationId,
    "opr"
);
prefixed_id!(
    /// Identifies one approval request.
    ApprovalId,
    "apv"
);
prefixed_id!(
    /// Identifies one provider workspace generation.
    GenerationId,
    "gen"
);
prefixed_id!(
    /// Identifies one workspace.
    WorkspaceId,
    "wks"
);
prefixed_id!(
    /// Identifies one billing organization.
    OrganizationId,
    "org"
);
prefixed_id!(
    /// Identifies one staged upload.
    UploadId,
    "upl"
);
prefixed_id!(
    /// Identifies one usage measurement.
    MeasurementId,
    "mea"
);
prefixed_id!(
    /// Identifies one telemetry export.
    ExportId,
    "exp"
);
prefixed_id!(
    /// Identifies one download grant.
    GrantId,
    "grt"
);
prefixed_id!(
    /// Identifies one spend reservation.
    ReservationId,
    "rsv"
);
prefixed_id!(
    /// Identifies one bounded query cursor.
    CursorId,
    "cur"
);
prefixed_id!(
    /// Identifies one journal effect.
    EffectId,
    "eff"
);
prefixed_id!(
    /// Identifies the immutable usage closure of one run.
    UsageClosureId,
    "ucl"
);
prefixed_id!(
    /// Identifies one queued unit of operation work.
    WorkId,
    "wrk"
);
prefixed_id!(
    /// Identifies one wrapped-key ownership edge.
    OwnerKeyEdgeId,
    "oke"
);

/// A caller-supplied idempotency key.
///
/// Printable ASCII, 1..=255 bytes. The value is opaque to the domain: it
/// selects a receipt envelope and never participates in an intent hash.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Longest accepted key.
    pub const MAX_BYTES: usize = 255;

    /// Validates and wraps a caller-supplied key.
    ///
    /// # Errors
    ///
    /// Returns [`IdempotencyKeyError`] when the key is empty, too long or
    /// contains a byte outside printable ASCII.
    pub fn parse(text: &str) -> Result<Self, IdempotencyKeyError> {
        if text.is_empty() {
            return Err(IdempotencyKeyError::Empty);
        }
        if text.len() > Self::MAX_BYTES {
            return Err(IdempotencyKeyError::TooLong { bytes: text.len() });
        }
        if let Some(position) = text
            .bytes()
            .position(|byte| !(0x21..=0x7e).contains(&byte))
        {
            return Err(IdempotencyKeyError::NotPrintableAscii { position });
        }
        Ok(Self(text.to_owned()))
    }

    /// Borrows the key text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdempotencyKey({})", self.0)
    }
}

/// Why an idempotency key was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdempotencyKeyError {
    /// The key was the empty string.
    #[error("idempotency key must not be empty")]
    Empty,
    /// The key exceeded [`IdempotencyKey::MAX_BYTES`].
    #[error("idempotency key must be at most {max} bytes, found {bytes}", max = IdempotencyKey::MAX_BYTES)]
    TooLong {
        /// Observed length in bytes.
        bytes: usize,
    },
    /// A byte outside printable ASCII appeared.
    #[error("idempotency key byte {position} is not printable ASCII")]
    NotPrintableAscii {
        /// Zero-based index of the offending byte.
        position: usize,
    },
}

/// Which deployment plane a record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Plane {
    /// The development plane.
    Dev,
    /// The production plane.
    Prd,
}

impl Plane {
    /// Stable lower-case token used in every canonical encoding.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prd => "prd",
        }
    }
}

/// An AWS-style region token, validated as `<letters>-<letters>-<digit>`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Region(String);

impl Region {
    /// Validates a region token.
    ///
    /// # Errors
    ///
    /// Returns [`RegionError`] when the token does not match
    /// `^[a-z]{2,}-[a-z]+-[1-9]$`.
    pub fn parse(text: &str) -> Result<Self, RegionError> {
        let mut parts = text.split('-');
        let (Some(area), Some(direction), Some(index), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(RegionError::Shape);
        };
        let lower = |segment: &str| {
            !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_lowercase())
        };
        if !lower(area) || area.len() < 2 || !lower(direction) {
            return Err(RegionError::Shape);
        }
        if index.len() != 1 || !matches!(index.as_bytes()[0], b'1'..=b'9') {
            return Err(RegionError::Shape);
        }
        Ok(Self(text.to_owned()))
    }

    /// Borrows the region token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Region({})", self.0)
    }
}

/// Why a region token was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegionError {
    /// The token did not have the required three-segment shape.
    #[error("region must look like `eu-west-1`")]
    Shape,
}

/// The six launch providers. Closed by A11-PROVIDERS; there is no other-arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderId {
    /// `openai`
    OpenAi,
    /// `anthropic`
    Anthropic,
    /// `deepseek`
    DeepSeek,
    /// `zai`
    ZAi,
    /// `moonshotai`
    MoonshotAi,
    /// `google`
    Google,
}

impl ProviderId {
    /// Stable public token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
            Self::ZAi => "zai",
            Self::MoonshotAi => "moonshotai",
            Self::Google => "google",
        }
    }
}

/// An IANA media type, validated as `type/subtype` with no parameters.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaType(String);

impl MediaType {
    /// Validates a media type.
    ///
    /// # Errors
    ///
    /// Returns [`MediaTypeError`] when the value is not exactly one
    /// `type/subtype` pair of restricted-name characters.
    pub fn parse(text: &str) -> Result<Self, MediaTypeError> {
        let mut parts = text.split('/');
        let (Some(kind), Some(subtype), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(MediaTypeError::Shape);
        };
        let token = |segment: &str| {
            !segment.is_empty()
                && segment.len() <= 127
                && segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'!' | b'#' | b'$' | b'&' | b'-' | b'^' | b'_' | b'.' | b'+')
                })
        };
        if token(kind) && token(subtype) {
            Ok(Self(text.to_owned()))
        } else {
            Err(MediaTypeError::Shape)
        }
    }

    /// Borrows the media type text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MediaType({})", self.0)
    }
}

/// Why a media type was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MediaTypeError {
    /// The value was not a bare lower-case `type/subtype` pair.
    #[error("media type must be a bare lower-case `type/subtype`")]
    Shape,
}

/// Wall-clock instant, milliseconds since the Unix epoch.
///
/// Integer milliseconds rather than a calendar type: every domain comparison is
/// a total order over a fixed quantum, and the canonical byte builders need a
/// fixed-width encoding that no formatting choice can move.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

/// Re-exported so the whole regional stack measures duration with one type.
pub use time::Duration;

impl Timestamp {
    /// The Unix epoch.
    pub const EPOCH: Self = Self(0);

    /// Builds an instant from milliseconds since the Unix epoch.
    #[must_use]
    pub const fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_millis(self) -> i64 {
        self.0
    }

    /// Adds a duration, returning `None` on overflow or a sub-millisecond
    /// duration that would round to zero movement in the wrong direction.
    #[must_use]
    pub fn checked_add(self, delta: Duration) -> Option<Self> {
        let millis = delta.whole_milliseconds();
        let millis = i64::try_from(millis).ok()?;
        self.0.checked_add(millis).map(Self)
    }

    /// Adds a duration, saturating at the representable bounds.
    #[must_use]
    pub fn saturating_add(self, delta: Duration) -> Self {
        self.checked_add(delta).unwrap_or(if delta.is_negative() {
            Self(i64::MIN)
        } else {
            Self(i64::MAX)
        })
    }

    /// Signed difference `self - earlier`.
    #[must_use]
    pub fn since(self, earlier: Self) -> Duration {
        Duration::milliseconds(self.0.saturating_sub(earlier.0))
    }

    /// Big-endian bytes, for canonical hashing.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({}ms)", self.0)
    }
}

/// RFC 8785 canonical bytes of one already-validated JSON document.
///
/// Constructed only by [`crate::canonical`], so there is exactly one
/// canonicalizer in the workspace.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalBytes(Vec<u8>);

impl CanonicalBytes {
    /// Wraps bytes the canonicalizer produced.
    pub(crate) fn from_canonicalizer(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Canonicalizes a JSON document.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError`] for malformed JSON, a duplicate object key, a
    /// non-integer number or a value outside the accepted profile.
    pub fn canonicalize(json: &str) -> Result<Self, CanonicalError> {
        crate::canonical::canonicalize(json)
    }

    /// Borrows the canonical bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Canonical byte length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the canonical form is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for CanonicalBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CanonicalBytes({} bytes)", self.0.len())
    }
}

/// A stable, non-reversible fingerprint of the calling principal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrincipalFingerprint([u8; 32]);

impl PrincipalFingerprint {
    /// Wraps a precomputed 32-byte fingerprint.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw fingerprint bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for PrincipalFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrincipalFingerprint(")?;
        for byte in &self.0[..4] {
            write!(f, "{byte:02x}")?;
        }
        f.write_str("…)")
    }
}

/// The HTTP methods that can carry an idempotent command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HttpMethod {
    /// `GET`
    Get,
    /// `POST`
    Post,
    /// `PUT`
    Put,
    /// `PATCH`
    Patch,
    /// `DELETE`
    Delete,
}

impl HttpMethod {
    /// The upper-case token used in the intent-hash scope.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// Durable limit identifiers the regional domains consult.
///
/// The numeric values are never compiled in; they are read through the
/// application's limits port so a per-workspace override is possible (D-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LimitId {
    /// Concurrently materialized agents per session.
    SessionMaterializedAgents,
    /// Maximum subagent nesting depth.
    SessionAgentDepth,
    /// Maximum page size for bounded queries.
    QueryPage,
    /// Maximum inline journal body bytes.
    JournalInlineBytes,
    /// Maximum expanded bundle bytes.
    ContentBundleExpandBytes,
    /// Maximum expanded bundle entries.
    ContentBundleExpandEntries,
    /// Maximum expanded bundle path bytes.
    ContentBundleExpandPathBytes,
}

impl LimitId {
    /// Stable durable token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionMaterializedAgents => "session.materialized_agents",
            Self::SessionAgentDepth => "session.agent_depth",
            Self::QueryPage => "query.page",
            Self::JournalInlineBytes => "session.journal_inline_bytes",
            Self::ContentBundleExpandBytes => "content.bundle_expand.bytes",
            Self::ContentBundleExpandEntries => "content.bundle_expand.entries",
            Self::ContentBundleExpandPathBytes => "content.bundle_expand.path_bytes",
        }
    }
}

/// The public error vocabulary. Closed: an unrecognized code is a decode error,
/// never a default (D-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorCode {
    /// The replayed identity carried a different intent.
    IdempotencyConflict,
    /// The replayed operation identity carried a different intent.
    OperationIdempotencyConflict,
    /// The approval already carries a decision.
    ApprovalAlreadyResolved,
    /// A bound approval field changed between request and decision.
    ApprovalBindingChanged,
    /// A referenced body is absent or checksum-mismatched.
    ContentMissing,
    /// The workspace is not in a live state.
    WorkspaceNotLive,
    /// The workspace requires activation first.
    WorkspaceActivationRequired,
    /// A supplied precondition did not hold.
    PreconditionFailed,
    /// A durable limit was exceeded.
    LimitExceeded,
    /// The requested byte range is not valid for the object.
    InvalidRange,
    /// A continuation cursor failed to decode.
    InvalidCursor,
    /// The billing account is paused.
    AccountPaused,
    /// A deletion already owns the subject.
    DeletionInProgress,
    /// The session has been purged.
    SessionDeleted,
    /// The command requires an idle session.
    SessionNotIdle,
    /// The command requires a non-terminal target.
    AlreadyTerminal,
    /// The referenced resource does not exist in this workspace.
    NotFound,
    /// A conflicting concurrent state was observed.
    Conflict,
    /// The request was structurally invalid.
    InvalidArgument,
    /// The operation cannot be cancelled in its current state.
    NotCancelable,
    /// A fenced write presented a stale fence.
    FenceConflict,
    /// A journal append violated the ordering algebra.
    JournalConflict,
    /// A secret is revoked or otherwise unusable.
    SecretUnusable,
    /// A staged upload is not in a usable state.
    UploadNotReady,
    /// The declarative plan itself was malformed; an internal fault.
    PlanRejected,
}

impl ErrorCode {
    /// Stable public token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::OperationIdempotencyConflict => "operation_idempotency_conflict",
            Self::ApprovalAlreadyResolved => "approval_already_resolved",
            Self::ApprovalBindingChanged => "approval_binding_changed",
            Self::ContentMissing => "content_missing",
            Self::WorkspaceNotLive => "workspace_not_live",
            Self::WorkspaceActivationRequired => "workspace_activation_required",
            Self::PreconditionFailed => "precondition_failed",
            Self::LimitExceeded => "limit_exceeded",
            Self::InvalidRange => "invalid_range",
            Self::InvalidCursor => "invalid_cursor",
            Self::AccountPaused => "account_paused",
            Self::DeletionInProgress => "deletion_in_progress",
            Self::SessionDeleted => "session_deleted",
            Self::SessionNotIdle => "session_not_idle",
            Self::AlreadyTerminal => "already_terminal",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::InvalidArgument => "invalid_argument",
            Self::NotCancelable => "not_cancelable",
            Self::FenceConflict => "fence_conflict",
            Self::JournalConflict => "journal_conflict",
            Self::SecretUnusable => "secret_unusable",
            Self::UploadNotReady => "upload_not_ready",
            Self::PlanRejected => "plan_rejected",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which idempotency vocabulary a conflict belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConflictCode {
    /// A request-level `Idempotency-Key` replay carried a different intent.
    Idempotency,
    /// An `Aex-Operation-Id` replay carried a different intent.
    OperationIdempotency,
}

impl ConflictCode {
    /// The public error code this conflict renders as.
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::Idempotency => ErrorCode::IdempotencyConflict,
            Self::OperationIdempotency => ErrorCode::OperationIdempotencyConflict,
        }
    }
}

/// Public projections. These are the shapes a customer sees; the internal
/// state machines are richer and project into them.
pub mod wire {
    /// Public session status.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum SessionStatus {
        /// No run is active.
        Idle,
        /// A run is active.
        Running,
        /// A run is blocked on an approval decision.
        AwaitingApproval,
        /// The session is in the recovery window.
        Trashed,
        /// The session is being purged.
        Purging,
    }

    /// Public run status.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum RunStatus {
        /// Admitted, not started.
        Queued,
        /// Executing.
        Running,
        /// Completed normally.
        Succeeded,
        /// Completed with an error.
        Failed,
        /// Exceeded its deadline.
        TimedOut,
        /// Cancelled by an operation.
        Cancelled,
        /// Fenced by a platform condition.
        Interrupted,
    }

    /// Public child-agent status. Seven states; the internal `Parked` projects
    /// to `Running` and the internal root-at-rest `Idle` has no projection.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum AgentStatus {
        /// Waiting for a scheduler claim.
        Queued,
        /// Claimed, not yet executing.
        Starting,
        /// Executing.
        Running,
        /// Winding down.
        Stopping,
        /// Finished normally.
        Completed,
        /// Finished with an error.
        Failed,
        /// Cancelled.
        Cancelled,
    }

    /// Public operation kind.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum OperationKind {
        /// Stop all work in a session.
        SessionStop,
        /// Persist the live workspace.
        SessionPersist,
        /// Clone a session.
        SessionClone,
        /// Discard the live workspace.
        WorkspaceDiscard,
        /// Rebind session credentials.
        CredentialRebind,
        /// Move a session to the recovery window.
        SessionTrash,
        /// Restore a session from the recovery window.
        SessionRestore,
        /// Purge a session.
        SessionPurge,
        /// Purge a workspace.
        WorkspacePurge,
        /// Export telemetry.
        TelemetryExport,
        /// Collect unreachable content.
        ContentGc,
    }

    /// Public operation status.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum OperationStatus {
        /// Accepted, not started.
        Queued,
        /// Executing.
        Running,
        /// Finished normally.
        Succeeded,
        /// Finished with an error.
        Failed,
        /// Cancelled before its commit latch.
        Cancelled,
    }
}

/// Stand-ins for `aex-internal-contracts`.
pub mod internal {
    use super::{AgentId, ApprovalId, EffectId, MessageId, RunId, SessionId, Timestamp, WorkspaceId};

    /// How an agent reached a terminal state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum AgentTerminalKind {
        /// Produced a result.
        Completed,
        /// Raised an error.
        Failed,
        /// Was cancelled.
        Cancelled,
    }

    /// The closed journal-entry vocabulary.
    ///
    /// Closed by the §8a cross-stream requirement: the authority fold matches
    /// it exhaustively, so a catch-all arm would silently admit an entry the
    /// fold cannot account for. Every arm carries exactly the identifiers the
    /// authority fold needs; the payload lives in the entry body and is
    /// interpreted only by `aex-brain-domain` (D-02).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum JournalEntryKind {
        /// The agent took its first execution step.
        AgentStarted,
        /// The agent parked without a lease.
        AgentParked,
        /// The agent resumed from a park.
        AgentResumed,
        /// The agent began winding down.
        AgentStopping,
        /// The agent reached a terminal state.
        AgentTerminal(AgentTerminalKind),
        /// A part was appended to an open message.
        MessagePartAppended {
            /// The message the part belongs to.
            message: MessageId,
        },
        /// A message was sealed.
        MessageSealed {
            /// The message that was sealed.
            message: MessageId,
        },
        /// An effect was prepared but not yet dispatched.
        EffectPrepared {
            /// The effect identity.
            effect: EffectId,
        },
        /// A prepared effect was dispatched.
        EffectOpened {
            /// The effect identity.
            effect: EffectId,
        },
        /// An open effect produced a receipt.
        EffectCompleted {
            /// The effect identity.
            effect: EffectId,
        },
        /// A prepared effect was abandoned before dispatch.
        EffectAbandoned {
            /// The effect identity.
            effect: EffectId,
        },
        /// An approval was requested.
        ApprovalRequested {
            /// The approval identity.
            approval: ApprovalId,
        },
        /// An approval was resolved.
        ApprovalResolved {
            /// The approval identity.
            approval: ApprovalId,
        },
        /// A child agent was spawned.
        ChildSpawned {
            /// The child agent.
            child: AgentId,
        },
        /// A child agent joined back into its parent.
        ChildJoined {
            /// The child agent.
            child: AgentId,
        },
        /// The retained-context byte total changed.
        ContextRetained {
            /// New retained byte total.
            bytes: u64,
        },
        /// A provider turn began.
        ProviderTurnStarted,
        /// A provider turn ended.
        ProviderTurnCompleted,
    }

    /// Why a wake hint was emitted after a commit.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum WakeReason {
        /// A user message was admitted.
        MessageAdmitted,
        /// An approval decision landed.
        ApprovalResolved,
        /// A child agent joined.
        ChildJoined,
        /// An open effect completed.
        EffectCompleted,
        /// A scheduled retry came due.
        ScheduledRetry,
        /// Workspace continuity was restored.
        ContinuityRestored,
    }

    /// What a native outbox event reports.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum OutboxEventKind {
        /// A session was created.
        SessionCreated,
        /// A run started.
        RunStarted,
        /// A run reached a terminal state.
        RunTerminal,
        /// A message was sealed.
        MessageSealed,
        /// A child agent changed status.
        AgentStatusChanged,
        /// An approval was requested.
        ApprovalRequested,
        /// An approval was resolved.
        ApprovalResolved,
        /// An operation reached a terminal state.
        OperationTerminal,
        /// A session entered the recovery window.
        SessionTrashed,
        /// A session left the recovery window.
        SessionRestored,
        /// A session was purged.
        SessionPurged,
    }

    /// One native outbox event. Delivery is a post-commit hint and can never
    /// gate the transaction that produced it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct OutboxEvent {
        /// Owning workspace.
        pub workspace: WorkspaceId,
        /// Owning session.
        pub session: SessionId,
        /// Monotone per-session sequence.
        pub seq: u64,
        /// What happened.
        pub kind: OutboxEventKind,
        /// The run the event belongs to, when there is one.
        pub run: Option<RunId>,
        /// The agent the event belongs to, when there is one.
        pub agent: Option<AgentId>,
        /// When the event was recorded.
        pub occurred_at: Timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HttpMethod, IdError, IdempotencyKey, MediaType, Plane, Region, SessionId, Timestamp,
        ID_BODY_LEN,
    };

    #[test]
    fn id_round_trips_through_its_canonical_form() {
        let id = SessionId::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let text = id.to_string();
        assert!(text.starts_with("ses_"));
        assert_eq!(text.len(), 4 + ID_BODY_LEN);
        assert_eq!(SessionId::parse(&text), Ok(id));
    }

    #[test]
    fn id_rejects_wrong_prefix_length_and_alphabet() {
        assert_eq!(
            SessionId::parse("run_00000000000000000000000000"),
            Err(IdError::Prefix { expected: "ses" })
        );
        assert_eq!(
            SessionId::parse("ses_0000"),
            Err(IdError::BodyLength { found: 4 })
        );
        assert_eq!(
            SessionId::parse("ses_0000000000000000000000000U"),
            Err(IdError::BodyCharacter { position: 25 })
        );
        assert_eq!(
            SessionId::parse("ses_80000000000000000000000000"),
            Err(IdError::BodyOverflow)
        );
    }

    #[test]
    fn maximum_id_fits_exactly() {
        let id = SessionId::from_u128(u128::MAX);
        assert_eq!(SessionId::parse(&id.to_string()), Ok(id));
    }

    #[test]
    fn scalar_newtypes_validate() {
        assert!(Region::parse("eu-west-1").is_ok());
        assert!(Region::parse("eu-west").is_err());
        assert!(Region::parse("EU-WEST-1").is_err());
        assert!(MediaType::parse("application/json").is_ok());
        assert!(MediaType::parse("application/json; charset=utf-8").is_err());
        assert!(IdempotencyKey::parse("").is_err());
        assert!(IdempotencyKey::parse("abc def").is_err());
        assert!(IdempotencyKey::parse("abc-def").is_ok());
        assert_eq!(Plane::Dev.as_str(), "dev");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
    }

    #[test]
    fn timestamp_arithmetic_is_millisecond_exact() {
        let base = Timestamp::from_unix_millis(1_000);
        let later = base.saturating_add(time::Duration::minutes(5));
        assert_eq!(later.unix_millis(), 301_000);
        assert_eq!(later.since(base), time::Duration::minutes(5));
        assert_eq!(Timestamp::from_unix_millis(i64::MAX)
            .saturating_add(time::Duration::hours(1))
            .unix_millis(), i64::MAX);
    }
}
