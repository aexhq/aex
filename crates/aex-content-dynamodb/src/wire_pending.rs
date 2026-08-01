//! Types this crate needs that a peer stream owns and has not landed yet.
//!
//! Every item names the path that replaces it. A different name at merge costs a
//! mechanical rename; the shape is what matters, and each shape is the one plan
//! 05 §4 declares.

use aex_wire::ids::{
    ContentHash, MeasurementId, OperationId, OrganizationId, SessionId, WorkspaceId,
};
use aex_wire::types::Timestamp;

// TODO(cross-stream): replaced by aex_content_domain::Blake3Digest at merge.
/// A `BLAKE3` digest, rendered `b3:<64 lowercase hex>`.
///
/// The Merkle page and root digest is `BLAKE3` while the body digest is
/// SHA-256, and that split is deliberate (D-01): the body digest is the
/// customer-visible `DownloadGrant.sha256` and S3's `ChecksumSHA256`, while a
/// page digest is internal, never exposed, and cheaper over many small pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blake3Digest([u8; 32]);

impl Blake3Digest {
    /// Wraps raw digest bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Hashes `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The bare lowercase hex, as it appears inside a key.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    /// The wire spelling, `b3:<64 lowercase hex>`.
    #[must_use]
    pub fn to_wire(self) -> String {
        format!("b3:{}", self.to_hex())
    }

    /// Parses the wire spelling.
    ///
    /// # Errors
    ///
    /// [`DigestError`] when the prefix is absent, the length is wrong, or a
    /// digit is not lowercase hexadecimal. A `sha256:` value never parses as a
    /// page digest and the reverse is equally impossible.
    pub fn parse(text: &str) -> Result<Self, DigestError> {
        let hex_text = text.strip_prefix("b3:").ok_or(DigestError::Prefix {
            expected: "b3:",
            found: first_token(text),
        })?;
        parse_hex(hex_text).map(Self)
    }
}

/// Why a digest could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DigestError {
    /// The wire prefix was absent or belonged to the other digest family.
    #[error("expected a `{expected}` digest but found `{found}`")]
    Prefix {
        /// The prefix the caller asked for.
        expected: &'static str,
        /// What the value started with.
        found: String,
    },
    /// The hexadecimal body was not 64 lowercase digits.
    #[error("a digest body must be 64 lowercase hexadecimal digits")]
    Body,
}

fn first_token(text: &str) -> String {
    text.split(':').next().unwrap_or_default().to_owned()
}

fn parse_hex(text: &str) -> Result<[u8; 32], DigestError> {
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DigestError::Body);
    }
    if text.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(DigestError::Body);
    }
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(text, &mut bytes).map_err(|_| DigestError::Body)?;
    Ok(bytes)
}

/// The bare lowercase hex of a body digest, as it appears inside a key.
///
/// `ContentHash::to_wire` renders `sha256:<hex>`; a key carries the hex alone
/// because the family is already fixed by the key template.
#[must_use]
pub fn body_hex(digest: &ContentHash) -> String {
    hex::encode(digest.as_bytes())
}

// TODO(cross-stream): replaced by aex_content_domain::SealedBytes at merge.
/// AEAD ciphertext plus the digest of the encryption context it is bound to.
///
/// The crate never sees a plaintext body: the content crypto adapter seals
/// before this crate is called and opens after it returns. `encContextDigest`
/// travels beside the ciphertext so a context mismatch is detected before a KMS
/// call is spent.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedBytes {
    /// The sealed bytes.
    pub ciphertext: Vec<u8>,
    /// SHA-256 of the canonical encryption context, lowercase hex.
    pub enc_context_digest: String,
}

impl std::fmt::Debug for SealedBytes {
    /// Prints the length and the context digest, never the ciphertext.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedBytes")
            .field(
                "ciphertext",
                &format_args!("<{} bytes>", self.ciphertext.len()),
            )
            .field("enc_context_digest", &self.enc_context_digest)
            .finish()
    }
}

// TODO(cross-stream): replaced by aex_content_domain::PinOwner at merge.
/// Who holds a pin.
///
/// The pin identity is the owning identifier, so removing a pin is a
/// `DeleteItem` on a known key and no reverse owner index or reference count
/// exists anywhere (R-DELETE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOwner {
    /// A session binding.
    Session(SessionId),
    /// A registry pointer, identified by `{kind}:{name}`.
    Registry {
        /// The registry kind.
        kind: String,
        /// The registered name.
        name: String,
    },
    /// A durable operation.
    Operation(OperationId),
    /// A telemetry or content export.
    Export(String),
    /// A listing cursor.
    Cursor(String),
    /// The garbage collector's own hold.
    Gc(String),
    /// One admitted message body, identified by `{session}#{message}`.
    Message {
        /// The owning session.
        session: SessionId,
        /// The message inside it.
        message: String,
    },
}

impl PinOwner {
    /// The `pin_kind` component.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Session(_) => "session",
            Self::Registry { .. } => "registry",
            Self::Operation(_) => "operation",
            Self::Export(_) => "export",
            Self::Cursor(_) => "cursor",
            Self::Gc(_) => "gc",
            Self::Message { .. } => "message",
        }
    }

    /// The `pin_id` component.
    #[must_use]
    pub fn id(&self) -> String {
        match self {
            Self::Session(session) => session.to_string(),
            Self::Registry { kind, name } => format!("{kind}:{name}"),
            Self::Operation(operation) => operation.to_string(),
            Self::Export(id) | Self::Cursor(id) | Self::Gc(id) => id.clone(),
            Self::Message { session, message } => format!("{session}#{message}"),
        }
    }
}

// TODO(cross-stream): replaced by aex_content_domain::GrantPlan at merge.
/// What a download grant authorises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantPlan {
    /// The opaque bearer token's SHA-256, lowercase hex. The token itself is
    /// never stored.
    pub token_sha256: String,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The body the grant reads.
    pub digest: ContentHash,
    /// The first authorised byte.
    pub range_start: u64,
    /// One past the last authorised byte.
    pub range_end_exclusive: u64,
    /// How many bytes the transfer measurement authorises.
    pub authorized_bytes: u64,
    /// The transfer measurement this grant is billed under.
    pub measurement: MeasurementId,
    /// The media type the redemption serves.
    pub media_type: String,
    /// When the grant stops being redeemable.
    pub expires_at: Timestamp,
}

// TODO(cross-stream): replaced by aex_content_domain::GcSweepPlan at merge.
/// One fenced sweep decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcSweepPlan {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The organization, re-checked after read.
    pub organization: OrganizationId,
    /// The body being swept.
    pub digest: ContentHash,
    /// The epoch the sweeper holds.
    pub epoch: u64,
    /// The epoch the descriptor was marked in.
    pub marked_epoch: u64,
}
