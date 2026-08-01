//! Hard-expiry, credential-bound central assertion verification and caching.

use std::collections::HashMap;
use std::sync::Arc;

use aex_internal_contracts::assertion::{
    AssertionAudience, AuthorizationAssertion, CredentialDigest, MAX_LIFETIME_MS,
    credential_bound_signing_input,
};
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use sha2::Digest as _;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

/// Credential bytes live only in this zeroizing, redacting wrapper.
#[derive(Clone)]
pub struct PresentedCredential {
    bytes: Zeroizing<Vec<u8>>,
    binding: [u8; 32],
}

impl PresentedCredential {
    /// Validates a bounded non-empty credential and derives its stable binding.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::MalformedCredential`] for an empty, oversized or
    /// control-character-bearing value.
    pub fn new(bytes: Vec<u8>) -> Result<Self, AuthFailure> {
        if bytes.is_empty() || bytes.len() > 4_096 || bytes.iter().any(u8::is_ascii_control) {
            return Err(AuthFailure::MalformedCredential);
        }
        let binding = sha2::Sha256::digest(&bytes).into();
        Ok(Self {
            bytes: Zeroizing::new(bytes),
            binding,
        })
    }

    /// Hash of the credential, safe for cache identity but not for logs.
    #[must_use]
    pub const fn binding(&self) -> [u8; 32] {
        self.binding
    }

    /// Exposes credential bytes only for the duration of the authority call.
    ///
    /// The callback shape prevents a borrowed view from outliving this
    /// zeroizing wrapper; implementations must still avoid copying the bytes.
    pub fn expose<R>(&self, callback: impl FnOnce(&[u8]) -> R) -> R {
        callback(&self.bytes)
    }
}

impl std::fmt::Debug for PresentedCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PresentedCredential(<redacted>)")
    }
}

/// Signed assertion transport plus the credential binding that must be covered.
#[derive(Clone)]
pub struct SignedAssertion {
    assertion: AuthorizationAssertion,
    key_id: String,
    credential_binding: [u8; 32],
    signature: Zeroizing<Vec<u8>>,
}

impl SignedAssertion {
    /// Constructs a bounded signed transport.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::MalformedAssertion`] for an invalid key id or
    /// signature bound.
    pub fn new(
        assertion: AuthorizationAssertion,
        key_id: impl Into<String>,
        credential_binding: [u8; 32],
        signature: Vec<u8>,
    ) -> Result<Self, AuthFailure> {
        let key_id = key_id.into();
        if key_id.is_empty()
            || key_id.len() > 128
            || key_id.bytes().any(|byte| byte.is_ascii_control())
            || signature.is_empty()
            || signature.len() > 512
        {
            return Err(AuthFailure::MalformedAssertion);
        }
        Ok(Self {
            assertion,
            key_id,
            credential_binding,
            signature: Zeroizing::new(signature),
        })
    }

    /// Which trust anchor is claimed to have signed this assertion.
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// The detached signature, for a verifier that was handed the transport
    /// rather than asked to check it.
    ///
    /// A signature is public data: it proves nothing without the message and the
    /// key, and it is already visible on the internal wire.
    #[must_use]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    /// The exact bytes a signature over this assertion must cover.
    ///
    /// One definition, published by the contract crate, so the issuing service
    /// and every verifying edge cannot disagree about what was signed.
    #[must_use]
    pub fn signing_input(&self) -> Vec<u8> {
        credential_bound_signing_input(
            &self.assertion,
            &CredentialDigest::new(self.credential_binding),
        )
    }
}

impl std::fmt::Debug for SignedAssertion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignedAssertion")
            .field("assertion", &"<redacted>")
            .field("key_id", &self.key_id)
            .field("credential_binding", &"<redacted>")
            .field("signature", &"<redacted>")
            .finish()
    }
}

/// Monotonic regional projection used to reject stale assertions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectedEpochs {
    /// API-key epoch.
    pub key: u64,
    /// Account-state epoch.
    pub account: u64,
    /// General revocation epoch.
    pub revocation: u64,
}

/// A structurally and cryptographically verified authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAuthorization {
    /// Verified assertion.
    pub assertion: AuthorizationAssertion,
    /// Binding of the credential used for this request.
    pub credential_binding: [u8; 32],
}

/// Verification-key provider. Concrete crypto stays in the composition root.
pub trait KeyVerifier: Send + Sync + 'static {
    /// Verifies `signature` under the exact named key and message.
    fn verify(&self, key_id: &str, message: &[u8], signature: &[u8]) -> bool;
}

/// Central assertion fetch port.
#[async_trait]
pub trait AssertionSource: Send + Sync + 'static {
    /// Fetches one current assertion for this credential.
    async fn obtain(
        &self,
        credential: &PresentedCredential,
    ) -> Result<SignedAssertion, AuthFailure>;
}

/// Why authentication failed closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthFailure {
    /// Credential grammar or bound was invalid.
    #[error("credential is malformed")]
    MalformedCredential,
    /// Signed transport grammar was invalid.
    #[error("assertion transport is malformed")]
    MalformedAssertion,
    /// Key id was unknown or signature invalid.
    #[error("assertion signature did not verify")]
    Signature,
    /// Assertion lifetime exceeded thirty seconds or was not forward.
    #[error("assertion lifetime is invalid")]
    Lifetime,
    /// Assertion was issued in the future.
    #[error("assertion is not active yet")]
    NotYetValid,
    /// Hard expiry was reached; equality is expired.
    #[error("assertion expired")]
    Expired,
    /// Audience did not name this service.
    #[error("assertion audience mismatch")]
    Audience,
    /// Assertion region did not name this host.
    #[error("assertion region mismatch")]
    Region,
    /// Assertion was minted for another credential.
    #[error("assertion credential binding mismatch")]
    CredentialBinding,
    /// A local epoch was newer than the assertion.
    #[error("assertion epoch is stale")]
    EpochRollback,
    /// Workspace-scoped regional assertion omitted a workspace.
    #[error("assertion has no workspace")]
    MissingWorkspace,
    /// Source was unavailable.
    #[error("assertion authority is unavailable")]
    SourceUnavailable,
    /// Configured byte budget cannot hold one entry.
    #[error("assertion cache byte budget is too small")]
    CacheBudget,
}

/// Verifies signature, hard time bounds, audience, region, binding and epochs.
///
/// # Errors
///
/// Returns the precise [`AuthFailure`] at the first failed verification stage.
pub fn verify<V: KeyVerifier>(
    verifier: &V,
    signed: &SignedAssertion,
    credential: &PresentedCredential,
    audience: AssertionAudience,
    region: Region,
    projected: ProjectedEpochs,
    now: Timestamp,
) -> Result<VerifiedAuthorization, AuthFailure> {
    let input = signed.signing_input();
    if !verifier.verify(&signed.key_id, &input, &signed.signature) {
        return Err(AuthFailure::Signature);
    }
    signed
        .assertion
        .validate()
        .map_err(|_| AuthFailure::Lifetime)?;
    let issued = signed.assertion.issued_at.unix_millis();
    let expires = signed.assertion.expires_at.unix_millis();
    let now = now.unix_millis();
    if expires.saturating_sub(issued) > MAX_LIFETIME_MS {
        return Err(AuthFailure::Lifetime);
    }
    if now < issued {
        return Err(AuthFailure::NotYetValid);
    }
    if now >= expires {
        return Err(AuthFailure::Expired);
    }
    if signed.assertion.audience != audience {
        return Err(AuthFailure::Audience);
    }
    if signed.assertion.region != region {
        return Err(AuthFailure::Region);
    }
    if signed.credential_binding != credential.binding {
        return Err(AuthFailure::CredentialBinding);
    }
    if signed.assertion.workspace.is_none() {
        return Err(AuthFailure::MissingWorkspace);
    }
    if signed.assertion.key_epoch.0 < projected.key
        || signed.assertion.account_epoch.0 < projected.account
        || signed.assertion.revocation_epoch.0 < projected.revocation
    {
        return Err(AuthFailure::EpochRollback);
    }
    Ok(VerifiedAuthorization {
        assertion: signed.assertion.clone(),
        credential_binding: signed.credential_binding,
    })
}

#[derive(Clone)]
struct Cached {
    authorization: VerifiedAuthorization,
    expires_at_ms: i64,
}

/// Byte-bounded credential cache with one refresh flight per credential binding.
pub struct VerifyingAssertionCache<S, V> {
    source: S,
    verifier: V,
    audience: AssertionAudience,
    region: Region,
    max_entries: usize,
    entries: Mutex<HashMap<[u8; 32], Cached>>,
    flights: Mutex<HashMap<[u8; 32], Arc<Mutex<()>>>>,
}

impl<S, V> VerifyingAssertionCache<S, V>
where
    S: AssertionSource,
    V: KeyVerifier,
{
    /// Constructs a cache. Each entry is charged a conservative 1 KiB.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::CacheBudget`] when one entry cannot fit.
    pub fn new(
        source: S,
        verifier: V,
        audience: AssertionAudience,
        region: Region,
        budget_bytes: usize,
    ) -> Result<Self, AuthFailure> {
        let max_entries = budget_bytes / 1_024;
        if max_entries == 0 {
            return Err(AuthFailure::CacheBudget);
        }
        Ok(Self {
            source,
            verifier,
            audience,
            region,
            max_entries,
            entries: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
        })
    }

    /// Returns a current cached assertion or performs one credential-keyed refresh.
    ///
    /// # Errors
    ///
    /// Returns a verification or source failure without extending an expired entry.
    pub async fn resolve(
        &self,
        credential: &PresentedCredential,
        projected: ProjectedEpochs,
        now: Timestamp,
    ) -> Result<VerifiedAuthorization, AuthFailure> {
        if let Some(hit) = self.cached(credential.binding, projected, now).await {
            return hit;
        }
        let flight = {
            let mut flights = self.flights.lock().await;
            Arc::clone(
                flights
                    .entry(credential.binding)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _guard = flight.lock().await;
        if let Some(hit) = self.cached(credential.binding, projected, now).await {
            return hit;
        }
        let signed = self.source.obtain(credential).await?;
        let authorization = verify(
            &self.verifier,
            &signed,
            credential,
            self.audience,
            self.region,
            projected,
            now,
        )?;
        let expires_at_ms = authorization.assertion.expires_at.unix_millis();
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
            credential.binding,
            Cached {
                authorization: authorization.clone(),
                expires_at_ms,
            },
        );
        Ok(authorization)
    }

    async fn cached(
        &self,
        binding: [u8; 32],
        projected: ProjectedEpochs,
        now: Timestamp,
    ) -> Option<Result<VerifiedAuthorization, AuthFailure>> {
        let mut entries = self.entries.lock().await;
        let entry = entries.get(&binding)?.clone();
        if now.unix_millis() >= entry.expires_at_ms {
            entries.remove(&binding);
            return None;
        }
        let assertion = &entry.authorization.assertion;
        if assertion.key_epoch.0 < projected.key
            || assertion.account_epoch.0 < projected.account
            || assertion.revocation_epoch.0 < projected.revocation
        {
            entries.remove(&binding);
            return None;
        }
        Some(Ok(entry.authorization))
    }
}
