//! The internal `central-authz` assertion exchange.
//!
//! There is exactly one authorization assertion in the platform: the
//! fixed-layout 323-byte Ed25519 envelope defined by
//! `aex_identity_domain::assertion`. This module owns the **exchange** around
//! it — the two requests a caller may make and the one answer it may receive —
//! and deliberately owns nothing about the envelope's own bytes, its claim set
//! or its verification policy.
//!
//! # Why the claim set is not re-described here
//!
//! An earlier revision of this module carried a JSON `AuthorizationAssertion`
//! plus a detached signature over its canonicalized bytes. That was a second
//! spelling of an artifact that already had one, and the two could not agree:
//! the JSON form carried neither the credential binding nor the account state,
//! so the binding had to be bolted on beside the claim set as a sibling field
//! and covered by a signing input invented for the purpose. A verifier then had
//! two things to get right — the claims and the field next to them — where the
//! binary envelope has one. The envelope is the wire form; this module carries
//! it as [`IssuedAssertion`] and never restates what is inside it.
//!
//! # Why the credential is named and never sent
//!
//! The stored verifier is `HMAC-SHA256(pepper, SHA-256(token))` — a MAC over a
//! *digest* rather than over the token — precisely so a regional edge can prove
//! which credential it holds without transmitting it. Both requests therefore
//! carry `presented_digest` and neither carries a credential. A request bearing
//! the token would authenticate identically and would additionally place every
//! customer secret in the central plane's logs, traces and memory.

use aex_wire::ids::{ApiKeyId, SessionId, UserId, WorkspaceId};
use aex_wire::types::Region;
use base64::Engine as _;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::SchemaVersion;

/// The longest an issued assertion may be, as text.
///
/// The envelope is fixed-length, so the encoded form is fixed-length too; this
/// bound is generous enough to survive a layout revision and small enough that a
/// hostile payload cannot make a decoder allocate. The **exact** length belongs
/// to the envelope's own decoder, which is the single owner of the layout.
pub const MAX_ASSERTION_TEXT_LEN: usize = 1_024;

/// Which regional service may accept an assertion.
///
/// This is the one audience vocabulary. The envelope's `audience_service` byte
/// is a codec over these variants, written in `aex_identity_domain::assertion`
/// where the rest of the layout lives; declaration order is therefore wire
/// order and reordering this enum is a wire change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionAudience {
    /// `regional-session-api`.
    RegionalSession,
    /// `tool-executor`.
    ///
    /// The one audience no customer credential ever reaches. Every audience
    /// above answers a request a customer made with a credential they hold, and
    /// `central-authz` mints the envelope over that presented credential. This
    /// one is minted inside `brain-mux` from a `FenceGuard`-proved activation,
    /// for a service whose only caller is `brain-mux`, so there is no presented
    /// credential and no customer on the path at all.
    ///
    ToolExec,
}

impl AssertionAudience {
    /// Every audience, in wire order.
    pub const ALL: [Self; 2] = [Self::RegionalSession, Self::ToolExec];

    /// Every audience a **customer credential** may be presented to.
    ///
    /// The complement of this list is not "audiences we have not got round to
    /// granting". It is the set of audiences whose envelopes are minted from
    /// something other than a presented credential, and a workspace key that
    /// claimed one would be claiming a principal kind it cannot be.
    pub const CUSTOMER_PRESENTABLE: [Self; 1] = [Self::RegionalSession];

    /// Whether a credential a customer presents may name this audience.
    #[must_use]
    pub const fn is_customer_presentable(self) -> bool {
        match self {
            Self::RegionalSession => true,
            Self::ToolExec => false,
        }
    }

    /// The deployable that accepts this audience.
    #[must_use]
    pub const fn deployable(self) -> &'static str {
        match self {
            Self::RegionalSession => "regional-session-api",
            Self::ToolExec => "tool-executor",
        }
    }

    /// The stored spelling, which is also the serialized one.
    ///
    /// One spelling for the wire and for the projected key row, so an audience
    /// written by the control plane and an audience read by a regional edge
    /// cannot come to disagree about how a variant is named.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegionalSession => "regional_session",
            Self::ToolExec => "tool_exec",
        }
    }

    /// Resolves a stored spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// The audiences one credential may be presented to.
///
/// A fixed-width bitset over [`AssertionAudience::ALL`] rather than a list: it
/// is `Copy`, it cannot hold a duplicate, and — the reason it exists — the empty
/// set is representable. A key row that names no audience therefore admits
/// **nothing**, which is the fail-closed direction; a list decoder that treated
/// "absent" as "unrestricted" would be the other one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AudienceSet(u8);

impl AudienceSet {
    /// No audience at all, which admits nothing.
    pub const EMPTY: Self = Self(0);

    /// Every audience in the vocabulary.
    pub const ALL: Self = Self((1 << AssertionAudience::ALL.len()) - 1);

    /// Every audience a credential a customer presents may be admitted to.
    ///
    /// This — not [`AudienceSet::ALL`] — is what a projected workspace-key row
    /// carries. The difference is one bit and it is a tenant-resolution
    /// property: the tool executor derives the tenant from the envelope alone,
    /// so an envelope minted over a credential the customer chose to present
    /// must never be one the executor would look at.
    pub const CUSTOMER_PRESENTABLE: Self = {
        let mut set = Self::EMPTY;
        let mut index = 0;
        while index < AssertionAudience::CUSTOMER_PRESENTABLE.len() {
            set = set.insert(AssertionAudience::CUSTOMER_PRESENTABLE[index]);
            index += 1;
        }
        set
    };

    /// The bit `audience` owns.
    ///
    /// The enum declares no explicit discriminants, so the discriminant of
    /// `AssertionAudience::ALL[n]` is `n`; `declaration_order_is_bit_order`
    /// asserts exactly that, which is what makes this cast a fact.
    const fn bit(audience: AssertionAudience) -> u8 {
        1_u8 << (audience as u8)
    }

    /// Whether this credential may be presented to `audience`.
    #[must_use]
    pub const fn contains(self, audience: AssertionAudience) -> bool {
        self.0 & Self::bit(audience) != 0
    }

    /// The set with `audience` added.
    #[must_use]
    pub const fn insert(self, audience: AssertionAudience) -> Self {
        Self(self.0 | Self::bit(audience))
    }

    /// Whether the set admits nothing.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every audience in the set, in wire order.
    pub fn iter(self) -> impl Iterator<Item = AssertionAudience> {
        AssertionAudience::ALL
            .into_iter()
            .filter(move |audience| self.contains(*audience))
    }

    /// The stored spellings, in wire order.
    #[must_use]
    pub fn to_strings(self) -> Vec<String> {
        self.iter()
            .map(|audience| audience.as_str().to_owned())
            .collect()
    }
}

impl FromIterator<AssertionAudience> for AudienceSet {
    fn from_iter<I: IntoIterator<Item = AssertionAudience>>(iter: I) -> Self {
        iter.into_iter().fold(Self::EMPTY, Self::insert)
    }
}

/// Why an exchange value was refused at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssertionError {
    /// The encoded envelope was empty, oversized, or not canonical base64url.
    #[error("an issued assertion is 1..={MAX_ASSERTION_TEXT_LEN} bytes of canonical base64url")]
    Encoding,
}

/// A 32-byte digest as it travels on the internal wire.
///
/// Canonical unpadded base64url in both directions: a padded or
/// standard-alphabet spelling of the same bytes is refused, so one digest has
/// exactly one encoding and a comparison of encoded forms is a comparison of
/// bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CredentialDigest([u8; 32]);

impl CredentialDigest {
    /// Wraps a computed digest.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    #[must_use]
    pub const fn get(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for CredentialDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CredentialDigest(<redacted:32 bytes>)")
    }
}

impl Serialize for CredentialDigest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for CredentialDigest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&text)
            .map_err(|_| D::Error::custom("expected canonical unpadded base64url"))?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map(Self)
            .map_err(|_| D::Error::custom("expected exactly 32 digest bytes"))
    }
}

/// A request for an assertion over a presented **workspace API key**.
///
/// This is the operation every regional edge runs on the hot path: a workspace
/// key is the only credential a customer presents to a regional host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolveWorkspaceKey {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The key metadata identity embedded in the presented token.
    pub key: ApiKeyId,
    /// `SHA-256` over the complete presented token.
    pub presented_digest: CredentialDigest,
    /// The region the calling edge is pinned to.
    ///
    /// Present so a key minted for another region is refused centrally as well
    /// as regionally. Neither check is a single point of failure.
    pub region: Region,
    /// Which regional service will accept the assertion.
    pub audience: AssertionAudience,
}

/// A request for a browser-session assertion over one workspace.
///
/// The sibling of [`ResolveWorkspaceKey`], and it issues **the same** envelope
/// through the same path rather than a second credential mechanism: same
/// person, same scopes, same role, different credential.
///
/// It carries `presented_digest` for the same reason its sibling does. A
/// session assertion that named no credential could be replayed by anyone who
/// obtained it, and no verifier could tell that it had been.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolveSessionForWorkspace {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The browser session being resolved.
    pub browser_session: SessionId,
    /// The signed-in person.
    pub user: UserId,
    /// The workspace the panel is about.
    pub workspace: WorkspaceId,
    /// `SHA-256` over the complete presented session token.
    pub presented_digest: CredentialDigest,
    /// The region the calling edge is pinned to.
    pub region: Region,
    /// Which regional service the assertion is for.
    pub audience: AssertionAudience,
}

/// One issued assertion envelope, in canonical unpadded base64url.
///
/// The envelope is self-describing — its own header carries a magic, a version
/// and an algorithm — so it needs no outer `schemaVersion` beside it. A second
/// version number would be a second thing that can disagree.
///
/// The exact length, layout and signature are checked by
/// `aex_identity_domain::assertion::Assertion::from_base64url`, which is the
/// single owner of the layout. This type only guarantees that the text is a
/// bounded, canonical base64url spelling of *some* bytes.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IssuedAssertion(String);

impl IssuedAssertion {
    /// Wraps an encoded envelope.
    ///
    /// # Errors
    ///
    /// Returns [`AssertionError::Encoding`] for an empty, oversized or
    /// non-canonical spelling. Canonicality is proved by re-encoding the decoded
    /// bytes and comparing, so a trailing character carrying non-zero unused
    /// bits is refused rather than silently accepted as a second spelling of the
    /// same envelope.
    pub fn new(text: impl Into<String>) -> Result<Self, AssertionError> {
        let text = text.into();
        if text.is_empty() || text.len() > MAX_ASSERTION_TEXT_LEN {
            return Err(AssertionError::Encoding);
        }
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let decoded = engine.decode(&text).map_err(|_| AssertionError::Encoding)?;
        if engine.encode(&decoded) != text {
            return Err(AssertionError::Encoding);
        }
        Ok(Self(text))
    }

    /// The encoded envelope, for a decoder that owns the layout.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for IssuedAssertion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "IssuedAssertion(<{} chars>)", self.0.len())
    }
}

impl Serialize for IssuedAssertion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IssuedAssertion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(text).map_err(D::Error::custom)
    }
}

/// Why `central-authz` issued no assertion.
///
/// A refusal is an **answer**, not a fault: the exchange succeeded and the
/// authority decided. An internal fault — an unreachable database, an envelope
/// that would not build — is reported as an invocation error instead, because a
/// caller must not report "your key is invalid" when the truth is "we could not
/// tell".
///
/// The vocabulary is deliberately the two outcomes a caller can act on
/// differently. Distinguishing "unknown key" from "wrong digest" from "revoked"
/// would tell an unauthenticated caller which of those it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionRefusal {
    /// The credential is not current: unknown, mismatched, revoked, lapsed,
    /// pinned to another region, or belonging to a workspace or organization
    /// that is not active. The caller answers `401`.
    NotAuthorized,
    /// The account state could not be established. Never downgraded to
    /// "active": asserting a state nobody could read is how a paused account
    /// keeps spending. The caller answers `503`.
    AccountStateUnavailable,
}

/// What `central-authz` answers either request with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AssertionResponse {
    /// One signed envelope, valid for at most thirty seconds.
    Issued {
        /// The envelope.
        assertion: IssuedAssertion,
    },
    /// The authority decided not to issue.
    Refused {
        /// Which decision it made.
        reason: AssertionRefusal,
    },
}
