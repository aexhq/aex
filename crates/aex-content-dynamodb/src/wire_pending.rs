//! Types this crate needs that a peer stream owns and has not landed yet.
//!
//! Every item names the path that replaces it. A different name at merge costs a
//! mechanical rename; the shape is what matters, and each shape is the one plan
//! 05 §4 declares.

use aex_wire::ids::{ContentHash, MeasurementId, OrganizationId, WorkspaceId};
use aex_wire::types::Timestamp;

/// The bare lowercase hex of a body digest, as it appears inside a key.
///
/// `ContentHash::to_wire` renders `sha256:<hex>`; a key carries the hex alone
/// because the family is already fixed by the key template.
#[must_use]
pub fn body_hex(digest: &ContentHash) -> String {
    hex::encode(digest.as_bytes())
}

// TODO(cross-stream): `aex-content-domain` publishes no sealed-bytes type — it is
// pure and never holds ciphertext. What it publishes is the identity of the
// ciphertext, `aex_content_domain::descriptor::CiphertextIdentity`, carried on a
// `descriptor::ContentDescriptor`.
/// One inline content body: AEAD ciphertext plus the digest of the encryption
/// context it is bound to.
///
/// The crate never sees a plaintext body: the content crypto adapter seals before
/// this crate is called and opens after it returns. `encContextDigest` travels
/// beside the ciphertext so a context mismatch is detected before a KMS call is
/// spent.
///
/// This is the **only** sealed shape here. The general `SealedBytes` it replaces
/// was historically broader. Session filesystem pages no longer exist; the
/// retained type is deliberately named for the registered content body it holds.
#[derive(Clone, PartialEq, Eq)]
pub struct InlineBody {
    /// The sealed bytes.
    pub ciphertext: Vec<u8>,
    /// SHA-256 of the canonical encryption context, lowercase hex.
    pub enc_context_digest: String,
}

impl std::fmt::Debug for InlineBody {
    /// Prints the length and the context digest, never the ciphertext.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InlineBody")
            .field(
                "ciphertext",
                &format_args!("<{} bytes>", self.ciphertext.len()),
            )
            .field("enc_context_digest", &self.enc_context_digest)
            .finish()
    }
}

// TODO(cross-stream): `aex-content-domain` publishes no persisted `PinOwner`;
// its pure `Pin` vocabulary does not encode DynamoDB key identities.
/// Who holds a pin.
///
/// The pin identity is the owning identifier, so removing a pin is a
/// `DeleteItem` on a known key and no reverse owner index or reference count
/// exists anywhere (R-DELETE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOwner {
    /// A registry pointer, identified by `{kind}:{name}`.
    Registry {
        /// The registry kind.
        kind: String,
        /// The registered name.
        name: String,
    },
}

impl PinOwner {
    /// The `pin_kind` component.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Registry { .. } => "registry",
        }
    }

    /// The `pin_id` component.
    #[must_use]
    pub fn id(&self) -> String {
        match self {
            Self::Registry { kind, name } => format!("{kind}:{name}"),
        }
    }
}

// TODO(cross-stream): the marker named the wrong crate. `aex-content-domain` has no
// grant vocabulary at all; the grant is `aex_workspace_domain::grant::DownloadGrant`
// with an `aex_workspace_domain::grant::GrantSubject`, a `grant::GrantPlacement` and a
// `grant::ByteRange`. This crate already decodes that type in `codec`, so the plan
// below and the real grant coexist.
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

// TODO(cross-stream): `aex-content-domain` publishes no sweep *plan*. It publishes the
// decision — `aex_content_domain::gc::SweepDecision` over a `gc::SweepCandidate` under
// a `gc::GcCondition`, fenced by a `gc::GcEpoch` — and leaves the write plan to the
// adapter, which is this struct.
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
