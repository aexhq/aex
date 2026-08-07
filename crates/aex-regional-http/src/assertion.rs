//! Hard-expiry, credential-bound central assertion verification and caching.
//!
//! The artifact this module verifies is the fixed-layout 323-byte Ed25519
//! envelope `aex_identity_domain::assertion` defines, and every cryptographic
//! and semantic check is that crate's. What lives here is the *edge's* half: the
//! zeroizing credential wrapper, the regional revocation projection the envelope
//! is checked against, and a bounded cache with one refresh flight per
//! credential.
//!
//! # Why there is no local `verify`
//!
//! There was one, over a JSON claim set with the credential binding carried
//! beside it as a sibling field. A verifier then had two things to get right —
//! the claims and the field next to them — and the covered bytes had to be
//! defined twice. The binding is now inside the signed body at a fixed offset,
//! so "which credential is this for" is not a separate thing that can be
//! tampered with, and there is exactly one implementation of the check.

use std::collections::HashMap;
use std::sync::Arc;

use aex_control_domain::epoch::{Epoch, EpochSubjectKind};
use aex_identity_domain::assertion::{
    Assertion, AssertionClaims, Audience, EpochProjection, Plane, VerificationInputs,
    VerificationKeySet, VerifyError, workspace_key_binding,
};
use aex_identity_domain::credential::PresentedDigest;
use aex_wire::ids::{ApiKeyId, WorkspaceApiKey};
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::Zeroizing;

/// The one credential a regional host accepts.
///
/// A workspace API key is the only credential a customer presents to a regional
/// endpoint, so the wrapper parses one at construction rather than carrying
/// arbitrary bytes and re-parsing at each of the three places that need the key
/// id. Anything else is [`AuthFailure::MalformedCredential`] before a byte of it
/// is used.
///
/// The plaintext lives only inside this zeroizing, redacting wrapper and is
/// never transmitted: what leaves the region is `(keyId, presentedDigest)`.
#[derive(Clone)]
pub struct PresentedCredential {
    bytes: Zeroizing<Vec<u8>>,
    key_id: ApiKeyId,
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

    /// The key identity as the envelope binds it: 16 raw bytes.
    #[must_use]
    pub fn key_id_raw(&self) -> Uuid {
        raw(self.key_id)
    }

    /// The regional endpoint the key is pinned to.
    #[must_use]
    pub const fn region(&self) -> Region {
        self.region
    }

    /// `SHA-256` over the complete token: what the central request carries.
    ///
    /// It is not a verifier. Without the pepper it proves nothing, and holding
    /// it does not let its holder authenticate.
    #[must_use]
    pub const fn digest(&self) -> &PresentedDigest {
        &self.digest
    }

    /// The binding the assertion for this credential must carry.
    ///
    /// Domain-separated by principal kind and key id, so an envelope minted for
    /// a browser session with the same token digest can never admit a request
    /// made with this key.
    #[must_use]
    pub fn expected_binding(&self) -> [u8; 32] {
        workspace_key_binding(raw(self.key_id), &self.digest)
    }

    /// The cache identity: the digest, which is safe to hold and index by.
    #[must_use]
    pub const fn cache_key(&self) -> &[u8; 32] {
        self.digest.as_bytes()
    }

    /// Exposes credential bytes only for the duration of a call that needs them.
    ///
    /// Nothing in the platform needs them today — the exchange sends a digest —
    /// but the accessor stays so that a future caller has one audited way in
    /// rather than a reason to hold the plaintext somewhere else.
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
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

/// The raw payload of a prefixed identifier, as the envelope binds it.
fn raw(id: ApiKeyId) -> Uuid {
    use aex_wire::ids::PrefixedId as _;
    Uuid::from_bytes(*id.uuid7().as_bytes())
}

/// The floor a subject this region cannot speak for is answered with.
///
/// Every epoch is at or below this, so any assertion carrying such a subject is
/// stale and refused. It is a value rather than an `Option` because
/// [`EpochProjection`] answers an `Epoch`, and the fail-closed answer has to be
/// expressible in that vocabulary.
const UNPROJECTABLE: Epoch = Epoch::new(u64::MAX);

/// The regional revocation floors an assertion is checked against.
///
/// The envelope carries `(kind, id, epoch)` slots rather than a flat epoch
/// triple, because the two principal kinds carry different subjects and the
/// region needs the subject **id** to consult its own projection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectedEpochs {
    /// The floor for the presented key.
    pub key: Epoch,
    /// The floor for the workspace the assertion names.
    pub workspace: Epoch,
    /// The floor for the owning organization's billing account.
    pub account: Epoch,
}

/// Stage one: the floor knowable from the credential alone.
///
/// A credential names its own key and nothing else. The workspace it belongs to
/// is a fact only the assertion carries, and the assertion may not be trusted
/// before it verifies — so the workspace and account floors cannot be applied
/// here without reading an unverified claim.
///
/// The unnamed subjects therefore answer `NEVER`, which **defers** rather than
/// admits: [`RegionalFloors`] is applied to every request afterwards, cache hit
/// or not, and it is the one that refuses an unprojectable subject. Nothing
/// reaches a handler on the strength of this stage alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialFloors {
    key: Epoch,
    key_id: Uuid,
}

impl CredentialFloors {
    /// Binds the key revocation floor to the key that was presented.
    #[must_use]
    pub const fn new(key: Epoch, key_id: Uuid) -> Self {
        Self { key, key_id }
    }
}

impl EpochProjection for CredentialFloors {
    fn projected(&self, kind: EpochSubjectKind, id: Uuid) -> Epoch {
        if kind == EpochSubjectKind::Key && id == self.key_id {
            self.key
        } else {
            Epoch::NEVER
        }
    }
}

/// Stage two: every floor, bound to the subject it belongs to.
///
/// A subject kind this region does **not** project answers [`UNPROJECTABLE`],
/// which makes any assertion carrying one stale and therefore refused. That is
/// the fail-closed direction and it is deliberate: the
/// `regional-authz-projection` holds nothing about a person or a membership, so
/// an assertion whose revocation depends on either cannot be checked here and
/// must not be admitted on the grounds that no reason to refuse it was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionalFloors {
    epochs: ProjectedEpochs,
    key_id: Uuid,
    workspace_id: Uuid,
    organization_id: Uuid,
}

impl RegionalFloors {
    /// Binds the floors to the three subjects this region projects.
    #[must_use]
    pub const fn new(
        epochs: ProjectedEpochs,
        key_id: Uuid,
        workspace_id: Uuid,
        organization_id: Uuid,
    ) -> Self {
        Self {
            epochs,
            key_id,
            workspace_id,
            organization_id,
        }
    }

    /// The floors themselves.
    #[must_use]
    pub const fn epochs(&self) -> ProjectedEpochs {
        self.epochs
    }

    /// Whether every subject the assertion carries is at or ahead of its floor.
    ///
    /// One call rather than a loop at each caller, so a deployable cannot check
    /// three of the subjects and forget the fourth.
    #[must_use]
    pub fn admits(&self, claims: &AssertionClaims) -> bool {
        claims.epochs.used().all(|slot| {
            !slot
                .epoch
                .is_stale_against(self.projected(slot.kind, slot.id))
        })
    }
}

impl EpochProjection for RegionalFloors {
    fn projected(&self, kind: EpochSubjectKind, id: Uuid) -> Epoch {
        match kind {
            EpochSubjectKind::Key if id == self.key_id => self.epochs.key,
            EpochSubjectKind::Workspace if id == self.workspace_id => self.epochs.workspace,
            EpochSubjectKind::Account if id == self.organization_id => self.epochs.account,
            // A subject this region cannot speak for, or one whose id does not
            // match the credential that was actually presented.
            _ => UNPROJECTABLE,
        }
    }
}

/// A structurally and cryptographically verified authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedAuthorization {
    /// The verified claims.
    pub claims: AssertionClaims,
    /// The binding of the credential used for this request.
    pub credential_binding: [u8; 32],
}

/// Central assertion fetch port.
#[async_trait]
pub trait AssertionSource: Send + Sync + 'static {
    /// Fetches one current assertion for this credential.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure`] for an unavailable authority or an answer that is
    /// not a well-formed envelope. An authority that *refused* is
    /// [`AuthFailure::Refused`] and never an outage.
    async fn obtain(&self, credential: &PresentedCredential) -> Result<Assertion, AuthFailure>;
}

/// Why authentication failed closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthFailure {
    /// Credential grammar or bound was invalid, or it was not a workspace key.
    #[error("credential is malformed")]
    MalformedCredential,
    /// The envelope was not a well-formed 323-byte assertion.
    #[error("assertion transport is malformed")]
    MalformedAssertion,
    /// The central authority declined to issue.
    #[error("the authority refused this credential")]
    Refused,
    /// The envelope did not verify, at the exact stage it failed.
    #[error("assertion verification failed: {0}")]
    Verification(VerifyError),
    /// Source was unavailable.
    #[error("assertion authority is unavailable")]
    SourceUnavailable,
    /// The account state could not be established centrally.
    #[error("account state is unavailable")]
    AccountStateUnavailable,
    /// Configured byte budget cannot hold one entry.
    #[error("assertion cache byte budget is too small")]
    CacheBudget,
}

impl From<VerifyError> for AuthFailure {
    fn from(error: VerifyError) -> Self {
        Self::Verification(error)
    }
}

/// Verifies one envelope for this edge.
///
/// Every check is `aex_identity_domain::assertion::verify`'s: signature first
/// under the exact `kid`, then the hard thirty-second lifetime re-checked
/// against the claim itself, then audience, region, credential binding and every
/// carried epoch against [`RegionalFloors`].
///
/// # Errors
///
/// Returns the [`VerifyError`] of the first rule that failed, plus a local
/// refusal for a principal kind this edge does not accept.
pub fn verify(
    assertion: &Assertion,
    keys: &VerificationKeySet,
    credential: &PresentedCredential,
    audience: Audience,
    floors: &CredentialFloors,
    now: Timestamp,
) -> Result<VerifiedAuthorization, AuthFailure> {
    let binding = credential.expected_binding();
    let now_ms = u64::try_from(now.unix_millis()).map_err(|_| VerifyError::NotYetValid)?;
    let claims = aex_identity_domain::assertion::verify(
        assertion.as_bytes(),
        keys,
        &VerificationInputs {
            now_ms,
            audience,
            credential_binding: &binding,
            projection: floors,
        },
    )?;
    Ok(VerifiedAuthorization {
        claims,
        credential_binding: binding,
    })
}

/// The plane an edge accepts assertions for.
#[must_use]
pub const fn plane(name: &str) -> Option<Plane> {
    match name.as_bytes() {
        b"dev" => Some(Plane::Dev),
        b"prd" => Some(Plane::Prd),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct Cached {
    authorization: VerifiedAuthorization,
    expires_at_ms: u64,
}

/// Byte-bounded credential cache with one refresh flight per credential.
///
/// The cache holds a *verified* assertion for at most its own thirty-second
/// lifetime, and the regional projection is read on every request regardless —
/// which is the only thing that makes caching an authorization decision safe.
pub struct VerifyingAssertionCache<S> {
    source: S,
    keys: VerificationKeySet,
    audience: Audience,
    max_entries: usize,
    entries: Mutex<HashMap<[u8; 32], Cached>>,
    flights: Mutex<HashMap<[u8; 32], Arc<Mutex<()>>>>,
}

impl<S: AssertionSource> VerifyingAssertionCache<S> {
    /// Constructs a cache. Each entry is charged a conservative 1 KiB.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::CacheBudget`] when one entry cannot fit.
    pub fn new(
        source: S,
        keys: VerificationKeySet,
        audience: Audience,
        budget_bytes: usize,
    ) -> Result<Self, AuthFailure> {
        let max_entries = budget_bytes / 1_024;
        if max_entries == 0 {
            return Err(AuthFailure::CacheBudget);
        }
        Ok(Self {
            source,
            keys,
            audience,
            max_entries,
            entries: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
        })
    }

    /// The audience every assertion this cache admits must name.
    #[must_use]
    pub const fn audience(&self) -> Audience {
        self.audience
    }

    /// Returns a current cached assertion or performs one credential-keyed refresh.
    ///
    /// # Errors
    ///
    /// Returns a verification or source failure without extending an expired
    /// entry and without admitting one whose epochs the region has moved past.
    pub async fn resolve(
        &self,
        credential: &PresentedCredential,
        floors: &CredentialFloors,
        now: Timestamp,
    ) -> Result<VerifiedAuthorization, AuthFailure> {
        let key = *credential.cache_key();
        if let Some(hit) = self.cached(key, floors, now).await {
            return Ok(hit);
        }
        let flight = {
            let mut flights = self.flights.lock().await;
            Arc::clone(
                flights
                    .entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _guard = flight.lock().await;
        if let Some(hit) = self.cached(key, floors, now).await {
            return Ok(hit);
        }
        let assertion = self.source.obtain(credential).await?;
        let authorization = verify(
            &assertion,
            &self.keys,
            credential,
            self.audience,
            floors,
            now,
        )?;
        let expires_at_ms = authorization.claims.expires_at_ms;
        let mut entries = self.entries.lock().await;
        if entries.len() >= self.max_entries {
            let oldest = entries
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at_ms)
                .map(|(binding, _)| *binding);
            if let Some(oldest) = oldest {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            key,
            Cached {
                authorization,
                expires_at_ms,
            },
        );
        Ok(authorization)
    }

    async fn cached(
        &self,
        key: [u8; 32],
        floors: &CredentialFloors,
        now: Timestamp,
    ) -> Option<VerifiedAuthorization> {
        let mut entries = self.entries.lock().await;
        let entry = *entries.get(&key)?;
        let now_ms = u64::try_from(now.unix_millis()).ok()?;
        if now_ms >= entry.expires_at_ms {
            entries.remove(&key);
            return None;
        }
        // A cached decision still loses to a revocation published a moment ago:
        // the credential floor is read on every request and re-applied here, and
        // the full subject-bound check runs outside this cache on every request
        // too, so an entry that was admissible when it was stored is dropped the
        // instant the region moves past it.
        for slot in entry.authorization.claims.epochs.used() {
            if slot
                .epoch
                .is_stale_against(floors.projected(slot.kind, slot.id))
            {
                entries.remove(&key);
                return None;
            }
        }
        Some(entry.authorization)
    }
}
