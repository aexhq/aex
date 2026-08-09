//! The presented workspace API key, and the in-region check that authenticates
//! it.
//!
//! # What this replaced, and why
//!
//! A regional edge used to authenticate by *asking*: it sent
//! `(keyId, presentedDigest)` to `central-authz`, received a 323-byte Ed25519
//! assertion, verified the signature locally and cached the result for thirty
//! seconds per credential. That put a synchronous Lambda on every API key
//! request and made one function a single point of failure for every regional
//! service in every region simultaneously.
//!
//! The check itself was never the part that needed to be central. It is a
//! peppered MAC of the presented key against a stored 32-byte verifier, and the
//! stored value is `HMAC-SHA256(pepper_v, SHA-256(token))`. Holding the pepper
//! and the verifier lets a holder **check** a presented key; it does not let
//! them mint one, because forging still needs a preimage against 256 bits of
//! CSPRNG secret. Replicating the verifier into the region therefore hands an
//! attacker who reaches the regional table nothing they could impersonate with.
//!
//! What the assertion carried besides the decision — the effective scopes, the
//! audience, the epochs, the account state — is carried by the projected rows
//! the admission snapshot already reads. See [`crate::edge::RegionalEdge::admit`]
//! for the stage that applies each one.

use aex_control_domain::epoch::Epoch;
use aex_identity_domain::credential::{Pepper, PepperVersion, PresentedDigest, Verifier};
use aex_wire::ids::{ApiKeyId, WorkspaceApiKey, WorkspaceId};
use aex_wire::types::Region;
use std::collections::BTreeMap;
use uuid::Uuid;
use zeroize::Zeroizing;

/// The one credential a regional host accepts.
///
/// A workspace API key is the only credential a customer presents to a regional
/// endpoint, so the wrapper parses one at construction rather than carrying
/// arbitrary bytes and re-parsing at each of the places that need the key id.
/// Anything else is [`AuthFailure::MalformedCredential`] before a byte of it is
/// used.
///
/// The plaintext lives only inside this zeroizing, redacting wrapper. It is now
/// never transmitted anywhere at all: the digest it derives is compared against
/// a replicated verifier in this process.
#[derive(Clone)]
pub struct PresentedCredential {
    bytes: Zeroizing<Vec<u8>>,
    key_id: ApiKeyId,
    workspace_id: WorkspaceId,
    region: Region,
    digest: PresentedDigest,
}

impl PresentedCredential {
    /// Validates a presented workspace API key and derives its stable digest.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::MalformedCredential`] for an empty, oversized,
    /// control-character-bearing or non-UTF-8 value, and for anything that is
    /// not a workspace API key.
    pub fn new(bytes: Vec<u8>) -> Result<Self, AuthFailure> {
        if bytes.is_empty() || bytes.len() > 4_096 || bytes.iter().any(u8::is_ascii_control) {
            return Err(AuthFailure::MalformedCredential);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| AuthFailure::MalformedCredential)?;
        let key = WorkspaceApiKey::parse(text).map_err(|_| AuthFailure::MalformedCredential)?;
        let digest = PresentedDigest::of(text);
        Ok(Self {
            key_id: key.key_id(),
            workspace_id: key.workspace_id(),
            region: key.region(),
            digest,
            bytes: Zeroizing::new(bytes),
        })
    }

    /// The key metadata identity embedded in the token.
    #[must_use]
    pub const fn key_id(&self) -> ApiKeyId {
        self.key_id
    }

    /// The workspace identity embedded in the token.
    ///
    /// This is a *claim*, not a fact: it names which rows the region reads, and
    /// the snapshot it reads them from is what decides whether the claim is
    /// true. A forged workspace id therefore costs one point-read miss, and the
    /// key authorization row's own workspace is what every later check uses.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// The key identity as 16 raw bytes.
    #[must_use]
    pub fn key_id_raw(&self) -> Uuid {
        raw(self.key_id)
    }

    /// The regional endpoint the key is pinned to.
    #[must_use]
    pub const fn region(&self) -> Region {
        self.region
    }

    /// `SHA-256` over the complete token: what the stored verifier is a MAC of.
    ///
    /// It is not a verifier. Without the pepper it proves nothing, and holding
    /// it does not let its holder authenticate.
    #[must_use]
    pub const fn digest(&self) -> &PresentedDigest {
        &self.digest
    }

    /// The stable principal identity a handler binds durable state to.
    ///
    /// Domain-separated by principal kind and key id, so two credentials can
    /// never share one cursor or one replay identity, and the value discloses
    /// nothing about the secret it is derived from.
    #[must_use]
    pub fn binding(&self) -> [u8; 32] {
        aex_identity_domain::assertion::workspace_key_binding(raw(self.key_id), &self.digest)
    }

    /// Exposes credential bytes only for the duration of a call that needs them.
    ///
    /// Nothing in the platform needs them — the check is over the digest — but
    /// the accessor stays so that a future caller has one audited way in rather
    /// than a reason to hold the plaintext somewhere else.
    pub fn expose<R>(&self, callback: impl FnOnce(&[u8]) -> R) -> R {
        callback(&self.bytes)
    }
}

impl std::fmt::Debug for PresentedCredential {
    /// Deliberately partial: the plaintext and its digest are omitted entirely
    /// rather than rendered as a placeholder field, so no format string anywhere
    /// can be persuaded to print them.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PresentedCredential")
            .field("key_id", &self.key_id)
            .field("workspace_id", &self.workspace_id)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

/// The raw payload of a prefixed identifier.
fn raw(id: ApiKeyId) -> Uuid {
    use aex_wire::ids::PrefixedId as _;
    Uuid::from_bytes(*id.uuid7().as_bytes())
}

/// The regional revocation floors, as the projection publishes them.
///
/// Three subjects and no more, because those are the only ones a region holds a
/// record for. A membership or a person is not projected here, which is why a
/// regional edge accepts one credential kind and nothing else.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectedEpochs {
    /// The floor for the presented key.
    pub key: Epoch,
    /// The floor for the workspace the key row names.
    pub workspace: Epoch,
    /// The floor for the owning organization's billing account.
    pub account: Epoch,
}

/// The stored proof a presented credential is checked against.
///
/// The version travels with the verifier rather than being assumed, because a
/// pepper rotation re-peppers rows one at a time: an edge that checked every
/// credential under the current pepper would refuse every credential minted
/// before the rotation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StoredVerifier {
    verifier: [u8; 32],
    pepper_version: u16,
}

impl StoredVerifier {
    /// Wraps a projected verifier and the pepper version it names.
    #[must_use]
    pub const fn new(verifier: [u8; 32], pepper_version: u16) -> Self {
        Self {
            verifier,
            pepper_version,
        }
    }

    /// Which pepper this verifier was computed under.
    #[must_use]
    pub const fn pepper_version(&self) -> u16 {
        self.pepper_version
    }
}

impl std::fmt::Debug for StoredVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredVerifier")
            .field("verifier", &"<redacted:32 bytes>")
            .field("pepper_version", &self.pepper_version)
            .finish()
    }
}

/// The most peppers one region holds at once.
///
/// A rotation needs two — the new pepper and the one existing verifiers were
/// computed under — and the bound is generous enough for a slow re-peppering.
/// It is the same number the issuing authority holds, because it is the same
/// document.
pub const MAX_PEPPERS: usize = 8;

/// The peppers a projected verifier may have been computed under.
///
/// Read once at cold start and never afterwards. A version this ring does not
/// hold makes a credential *unverifiable* rather than invalid, which is why
/// [`AuthFailure::UnknownPepper`] is a `503` and not a `401`.
#[derive(Clone)]
pub struct PepperRing {
    peppers: BTreeMap<u16, Pepper>,
}

impl std::fmt::Debug for PepperRing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PepperRing")
            .field("versions", &self.peppers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl PepperRing {
    /// Builds a ring over the decoded peppers.
    ///
    /// # Errors
    ///
    /// Returns [`RingError`] for an empty or over-long ring. Both stop the
    /// process: an edge that holds no pepper can verify nothing, and would
    /// otherwise report ready while refusing every credential it is shown.
    pub fn new(peppers: BTreeMap<u16, Pepper>) -> Result<Self, RingError> {
        if peppers.is_empty() {
            return Err(RingError::Empty);
        }
        if peppers.len() > MAX_PEPPERS {
            return Err(RingError::TooMany {
                found: peppers.len(),
            });
        }
        Ok(Self { peppers })
    }

    /// How many peppers the ring holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peppers.len()
    }

    /// Whether the ring holds none, which start-up already refuses.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peppers.is_empty()
    }

    /// The versions held, ascending.
    #[must_use]
    pub fn versions(&self) -> Vec<u16> {
        self.peppers.keys().copied().collect()
    }

    /// Whether the presented credential matches the projected verifier.
    ///
    /// The comparison is [`aex_identity_domain::credential::verify`]'s, which is
    /// constant time: a timing oracle over a 32-byte MAC is a credential oracle.
    /// No cryptography is authored here — the MAC, its domain separation and the
    /// comparison all belong to the crate that also mints the credential.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::UnknownPepper`] when the row names a version this
    /// process did not load — the credential is unverifiable, not invalid, so
    /// answering "your key is wrong" would be false — and
    /// [`AuthFailure::VerifierMismatch`] when the presented secret does not
    /// produce the stored verifier.
    pub fn admits(
        &self,
        credential: &PresentedCredential,
        stored: &StoredVerifier,
    ) -> Result<(), AuthFailure> {
        let pepper =
            self.peppers
                .get(&stored.pepper_version)
                .ok_or(AuthFailure::UnknownPepper {
                    version: stored.pepper_version,
                })?;
        let matches = aex_identity_domain::credential::verify(
            pepper,
            credential.digest(),
            &Verifier::from_bytes(stored.verifier),
        );
        if matches {
            Ok(())
        } else {
            Err(AuthFailure::VerifierMismatch)
        }
    }

    /// The pepper a verifier at `version` was computed under.
    #[must_use]
    pub fn get(&self, version: PepperVersion) -> Option<&Pepper> {
        self.peppers.get(&version.get())
    }
}

/// Why a decoded pepper ring was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RingError {
    /// The ring declared no pepper, which would verify nothing.
    #[error("the ring declares no pepper")]
    Empty,
    /// The ring was past the bound.
    #[error("the ring declares {found} peppers; at most {MAX_PEPPERS} are held")]
    TooMany {
        /// How many were declared.
        found: usize,
    },
}

/// Why authentication failed closed.
///
/// Three of the four arms are the customer's problem and answer `401`; the
/// fourth is ours and answers `503`. Keeping them apart is the whole point: a
/// caller told "your key is invalid" will rotate a key that was never wrong,
/// and a caller told "try again" will retry a key that will never work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthFailure {
    /// Credential grammar or bound was invalid, or it was not a workspace key.
    #[error("credential is malformed")]
    MalformedCredential,
    /// The presented secret does not produce the projected verifier.
    #[error("the presented credential does not match the projected verifier")]
    VerifierMismatch,
    /// The credential was presented to an edge its row does not name.
    #[error("the credential was presented to an edge it does not authorize")]
    AudienceMismatch,
    /// The projected row names a pepper version this process did not load.
    ///
    /// Deliberately not a refusal: the credential is *unverifiable*, and
    /// answering `unauthenticated` would tell a customer their key is wrong when
    /// the truth is that this process cannot check it.
    #[error("pepper version {version} is not held by this process")]
    UnknownPepper {
        /// The version the row named.
        version: u16,
    },
}
