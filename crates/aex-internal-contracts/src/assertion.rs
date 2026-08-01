//! The credential-bound central authorization assertion.
//!
//! `central-authz` signs [`signing_input`]; `aex-regional-http` verifies it. This
//! crate owns the canonical bytes and nothing else: the KMS key, the signature
//! algorithm and the verification policy belong to the issuing and verifying
//! services.
//!
//! The lifetime is hard: an assertion may live at most thirty seconds, and the
//! constructor enforces it. A regional edge may reuse one only inside that
//! window and only while its epochs are not older than the local monotonic
//! revocation projection.

use aex_wire::idempotency::{PrincipalKind, PrincipalScope};
use aex_wire::ids::{ApiKeyId, OrganizationId, SessionId, UserId, WorkspaceId};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{Region, Timestamp};
use base64::Engine as _;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Epoch, SchemaVersion};

/// The longest an assertion may live.
pub const MAX_LIFETIME_MS: i64 = 30_000;

/// Who an assertion is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionAudience {
    /// The regional session API.
    RegionalSession,
    /// The regional secret API.
    RegionalSecret,
    /// The regional observation API.
    RegionalObservation,
    /// The regional OTLP admission API.
    RegionalOtlp,
    /// The regional stream service.
    RegionalStream,
}

/// Why an assertion could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssertionError {
    /// The requested lifetime exceeded the hard bound.
    #[error("an assertion may live at most {MAX_LIFETIME_MS} ms")]
    LifetimeTooLong,
    /// `expires_at` was not after `issued_at`.
    #[error("an assertion must expire after it is issued")]
    NotForwardInTime,
    /// The detached signature was empty or past the bound.
    #[error("a signature is 1..={MAX_SIGNATURE_BYTES} bytes")]
    SignatureBound,
}

/// The 30-second credential-bound central assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthorizationAssertion {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// Who the credential establishes.
    pub principal: PrincipalScope,
    /// Which principal kind issued it; `user_session` and `account` share a path.
    pub principal_kind: PrincipalKind,
    /// The workspace, when the request is workspace-scoped.
    pub workspace: Option<WorkspaceId>,
    /// The owning organization.
    pub organization: OrganizationId,
    /// The region the assertion is valid in.
    pub region: Region,
    /// The scopes the credential actually carries.
    pub scopes: ScopeSet,
    /// The API key epoch at issue time.
    pub key_epoch: Epoch,
    /// The account epoch at issue time.
    pub account_epoch: Epoch,
    /// The revocation epoch at issue time.
    pub revocation_epoch: Epoch,
    /// When it was issued.
    pub issued_at: Timestamp,
    /// When it stops verifying.
    pub expires_at: Timestamp,
    /// Which service may accept it.
    pub audience: AssertionAudience,
}

impl AuthorizationAssertion {
    /// Checks the hard lifetime bound.
    ///
    /// # Errors
    ///
    /// Returns [`AssertionError`] when the assertion does not expire after it
    /// was issued, or lives longer than [`MAX_LIFETIME_MS`].
    pub fn validate(&self) -> Result<(), AssertionError> {
        let lifetime = self.expires_at.unix_millis() - self.issued_at.unix_millis();
        if lifetime <= 0 {
            return Err(AssertionError::NotForwardInTime);
        }
        if lifetime > MAX_LIFETIME_MS {
            return Err(AssertionError::LifetimeTooLong);
        }
        Ok(())
    }
}

/// The domain-separated signing input.
///
/// The prefix is part of the bytes so a signature over an assertion can never be
/// replayed as a signature over anything else the same key signs.
///
/// # Panics
///
/// Never: the assertion is composed of types that always serialize.
#[must_use]
pub fn signing_input(assertion: &AuthorizationAssertion) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(512);
    bytes.extend_from_slice(b"aex:authorization-assertion:v1\x1f");
    bytes.extend_from_slice(
        &aex_wire::to_jcs_bytes(assertion).expect("an assertion always canonicalizes"),
    );
    bytes
}

/// The domain-separated signing input for a **credential-bound** assertion.
///
/// [`signing_input`] covers the claim set alone, which is enough only where the
/// transport itself establishes which credential was presented. The regional
/// edge has no such transport: it receives an assertion in a response body, so
/// the binding has to be inside the signed bytes or an assertion minted for one
/// key could be replayed with another.
///
/// This is the one definition of those bytes. `central-authz` signs it and the
/// regional edge verifies it; two spellings of "the covered bytes" is exactly
/// the failure this function exists to prevent.
///
/// # Panics
///
/// Never: both members are composed of types that always canonicalize.
#[must_use]
pub fn credential_bound_signing_input(
    assertion: &AuthorizationAssertion,
    credential_binding: &CredentialDigest,
) -> Vec<u8> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Bound<'a> {
        assertion: &'a AuthorizationAssertion,
        credential_binding: &'a CredentialDigest,
    }
    let mut bytes = Vec::with_capacity(640);
    bytes.extend_from_slice(b"aex:credential-bound-authorization-assertion:v1\x1f");
    bytes.extend_from_slice(
        &aex_wire::to_jcs_bytes(&Bound {
            assertion,
            credential_binding,
        })
        .expect("a bound assertion always canonicalizes"),
    );
    bytes
}

/// A request for a browser-session assertion over one workspace.
///
/// The clients stream found that `central-authz` could resolve an account token
/// for a workspace but had no browser-session equivalent, which left every
/// regional dashboard panel unimplementable. This is that operation, and it
/// issues the same 30-second assertion through the same path rather than a
/// second credential mechanism.
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
    /// Which regional service the assertion is for.
    pub audience: AssertionAudience,
}

/// What `resolve_session_for_workspace` answers with.
///
/// # Known gap
///
/// This envelope carries the claim set and nothing that authenticates it, so a
/// regional edge cannot verify it: verification needs the signing key id, the
/// credential binding the signature covers, and the detached signature itself.
/// [`SignedAssertionEnvelope`] is the shape that does carry them, and the
/// browser-session operation should answer with it once the identity stream
/// confirms the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedSessionAssertion {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The assertion, with principal kind `user_session`.
    pub assertion: AuthorizationAssertion,
}

/// The longest a detached assertion signature may be.
///
/// Ed25519 produces 64 bytes. The bound is generous enough to survive an
/// algorithm change and small enough that a hostile payload cannot make the
/// verifier allocate.
pub const MAX_SIGNATURE_BYTES: usize = 512;

/// The longest a signing-key identity may be.
pub const MAX_KEY_ID_BYTES: usize = 128;

/// A 32-byte digest as it travels on the internal wire.
///
/// Canonical unpadded base64url in both directions: a padded or standard-alphabet
/// spelling of the same bytes is refused, so one digest has exactly one encoding
/// and a comparison of encoded forms is a comparison of bytes.
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

/// A detached assertion signature as it travels on the internal wire.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AssertionSignature(Vec<u8>);

impl AssertionSignature {
    /// Wraps a bounded non-empty signature.
    ///
    /// # Errors
    ///
    /// Returns [`AssertionError::SignatureBound`] for an empty signature or one
    /// past [`MAX_SIGNATURE_BYTES`].
    pub fn new(bytes: Vec<u8>) -> Result<Self, AssertionError> {
        if bytes.is_empty() || bytes.len() > MAX_SIGNATURE_BYTES {
            return Err(AssertionError::SignatureBound);
        }
        Ok(Self(bytes))
    }

    /// The raw signature.
    #[must_use]
    pub fn get(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for AssertionSignature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "AssertionSignature(<redacted:{} bytes>)",
            self.0.len()
        )
    }
}

impl Serialize for AssertionSignature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for AssertionSignature {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&text)
            .map_err(|_| D::Error::custom("expected canonical unpadded base64url"))?;
        Self::new(bytes)
            .map_err(|_| D::Error::custom("expected 1..={MAX_SIGNATURE_BYTES} signature bytes"))
    }
}

/// A request for an assertion over a presented **workspace API key**.
///
/// This is the sibling of [`ResolveSessionForWorkspace`], and it is the operation
/// every regional edge runs on the hot path: a workspace key is the only
/// credential a customer presents to a regional host.
///
/// # Why the key is named by id and digest rather than sent verbatim
///
/// The stored verifier is `HMAC-SHA256(pepper, SHA-256(token))`, keyed over a
/// digest rather than over the token, precisely so the customer's plaintext key
/// never has to leave the region it was presented in. Sending `{key,
/// presentedDigest}` proves the same thing to `central-authz` — it recomputes the
/// MAC over the digest and compares in constant time — while keeping the secret
/// regional. A request carrying the token itself would authenticate identically
/// and would additionally place every customer key in the central plane's logs,
/// traces and memory.
///
/// The digest is `SHA-256` over the complete token's UTF-8 bytes, which is the
/// same value the regional edge already derives as its credential binding. That
/// is not a coincidence and is what lets the response's `credentialBinding` be
/// compared against the credential actually presented.
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

/// A signed assertion, as the internal wire carries it.
///
/// The claim set alone is not verifiable, so this envelope carries the three
/// things a verifier needs beside it: which trust anchor signed, which credential
/// the signature is bound to, and the signature itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SignedAssertionEnvelope {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The claim set.
    pub assertion: AuthorizationAssertion,
    /// Which trust anchor signed it.
    pub key_id: String,
    /// The credential binding the signature covers.
    ///
    /// A verifier compares this against the credential it actually received, so
    /// an assertion minted for one key can never admit a request made with
    /// another.
    pub credential_binding: CredentialDigest,
    /// The detached signature over [`signing_input`] extended by the binding.
    pub signature: AssertionSignature,
}
