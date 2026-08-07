//! Provider-credential resolution and in-process custody (plan 08 §7).
//!
//! # What this module does not own
//!
//! The `pcr_` binding directory itself belongs to `aex-secret-domain` plus
//! `aex-secret-custody-dynamodb` in the `regional-secret-custody` table
//! (OD-23). A `BYOK` key is a workspace secret; giving it a second custody home
//! would create a second encryption authority. This module defines the two
//! ports it needs, consumes them, and ships [`DenyAllCredentialDirectory`] as
//! the typed placeholder until the owning stream lands.
//!
//! There is deliberately **no** plaintext-from-environment path. Not "disabled
//! by default" — absent, with a test asserting no such constructor exists.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aex_wire::ids::{ProviderCredentialId, WorkspaceId};
use aex_wire::provider::ProviderId;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::transport::AuthScheme;
use crate::wire_pending::{
    BoxFuture, CiphertextRef, EncryptionContext, RevocationEpoch, SourceGeneration,
};

/// An immutable revision of a binding. Rotation creates a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialRevision(pub u64);

/// Whether a binding may still be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    /// Usable.
    Ready,
    /// Revoked; the epoch moved past every pin taken against it.
    Revoked,
    /// Deleted.
    Deleted,
}

/// One workspace-scoped provider credential binding.
///
/// Immutable: every field is fixed at creation, and a rotation produces a new
/// `revision` rather than mutating this one. That is what lets a session pin a
/// binding and get the same behaviour for its whole life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialBinding {
    /// The stable `pcr_` identity.
    pub id: ProviderCredentialId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Which provider the binding is for. A mismatch is a typed error, never a
    /// silent substitution.
    pub provider: ProviderId,
    /// The immutable revision.
    pub revision: CredentialRevision,
    /// The secret-source generation the ciphertext belongs to.
    pub generation: SourceGeneration,
    /// The workspace revocation epoch at the time this binding was read.
    pub revocation_epoch: RevocationEpoch,
    /// Whether this is the workspace's default binding for its provider.
    pub is_default: bool,
    /// Whether it may still be used.
    pub state: BindingState,
    /// Where the ciphertext lives. Never the ciphertext itself.
    pub ciphertext: CiphertextRef,
    /// The encryption context a decrypt must present.
    pub context: EncryptionContext,
}

/// What a session pinned at admission.
///
/// Runtime behaviour never depends on a mutable "current default": the session
/// records the exact binding, revision and generation it was admitted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCredentialPin {
    /// Which binding.
    pub binding: ProviderCredentialId,
    /// Which revision.
    pub revision: CredentialRevision,
    /// Which generation.
    pub generation: SourceGeneration,
    /// The revocation epoch observed at admission. A later epoch overrides the
    /// pin and fails the next dispatch before send.
    pub epoch_at_admission: RevocationEpoch,
}

/// The non-secret reference a receipt carries.
///
/// It names the binding; it can hold neither ciphertext nor plaintext, so an
/// exported receipt is safe by construction rather than by review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialBindingRef {
    /// Which binding.
    pub id: ProviderCredentialId,
    /// Which revision.
    pub revision: CredentialRevision,
    /// Which generation.
    pub generation: SourceGeneration,
}

impl From<&ProviderCredentialBinding> for CredentialBindingRef {
    fn from(binding: &ProviderCredentialBinding) -> Self {
        Self {
            id: binding.id,
            revision: binding.revision,
            generation: binding.generation,
        }
    }
}

/// Why credential resolution failed. Every arm is `DispatchProof::NotSent`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialResolveError {
    /// No such binding.
    #[error("no provider credential binding matched")]
    NotFound {
        /// What was asked for, where the caller named one.
        requested: Option<ProviderCredentialId>,
    },
    /// The binding is for another provider.
    #[error("binding is for `{binding}`, not `{requested}`")]
    ProviderMismatch {
        /// The binding's provider.
        binding: ProviderId,
        /// The requested provider.
        requested: ProviderId,
    },
    /// The workspace revocation epoch moved past the pin.
    #[error("the credential was revoked: admitted at {admitted:?}, now {current:?}")]
    Revoked {
        /// The epoch the session pinned.
        admitted: RevocationEpoch,
        /// The epoch now in force.
        current: RevocationEpoch,
    },
    /// The binding was deleted.
    #[error("the credential binding was deleted")]
    Deleted,
    /// The workspace has no default binding for the provider.
    #[error("workspace has no default credential for `{provider}`")]
    NoDefault {
        /// Which provider.
        provider: ProviderId,
    },
    /// More than one default. A data-invariant violation, alarmed.
    #[error("workspace has {count} default credentials for one provider")]
    AmbiguousDefault {
        /// How many were found.
        count: u16,
    },
    /// The ciphertext did not decrypt.
    #[error("the credential did not decrypt")]
    DecryptFailed,
    /// The directory or the key store could not be reached.
    #[error("the credential directory could not be reached")]
    Transport,
}

impl CredentialResolveError {
    /// The public wire code this failure renders as.
    #[must_use]
    pub const fn error_code(&self) -> aex_wire::ErrorCode {
        match self {
            Self::Revoked { .. } => aex_wire::ErrorCode::ProviderCredentialRevoked,
            _ => aex_wire::ErrorCode::ProviderCredentialNotFound,
        }
    }
}

/// The binding directory this crate consumes.
///
/// `TODO(cross-stream): implemented by the regional secret stream over the
/// `regional-secret-custody` table (OD-23).`
pub trait ProviderCredentialDirectory: Send + Sync + 'static {
    /// Resolves a binding, either by explicit id or by the workspace default.
    fn resolve(
        &self,
        workspace: WorkspaceId,
        provider: ProviderId,
        id: Option<ProviderCredentialId>,
    ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>>;

    /// The workspace's current revocation epoch.
    fn current_epoch(
        &self,
        workspace: WorkspaceId,
        id: ProviderCredentialId,
    ) -> BoxFuture<'_, Result<RevocationEpoch, CredentialResolveError>>;
}

/// The decryptor this crate consumes.
///
/// `TODO(cross-stream): implemented by the regional secret stream over `KMS`.`
pub trait ProviderCredentialDecryptor: Send + Sync + 'static {
    /// Decrypts a binding's ciphertext immediately before dispatch.
    fn decrypt<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
    ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>>;
}

/// The typed placeholder until the owning stream lands.
///
/// It refuses every resolution. That is the correct closed default: the
/// alternative — reading a plaintext key from the environment — is exactly the
/// path this design exists to remove, so it is not implemented at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllCredentialDirectory;
impl ProviderCredentialDirectory for DenyAllCredentialDirectory {
    fn resolve(
        &self,
        _workspace: WorkspaceId,
        _provider: ProviderId,
        id: Option<ProviderCredentialId>,
    ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>> {
        Box::pin(async move { Err(CredentialResolveError::NotFound { requested: id }) })
    }

    fn current_epoch(
        &self,
        _workspace: WorkspaceId,
        id: ProviderCredentialId,
    ) -> BoxFuture<'_, Result<RevocationEpoch, CredentialResolveError>> {
        Box::pin(async move {
            Err(CredentialResolveError::NotFound {
                requested: Some(id),
            })
        })
    }
}

/// A decryptor that refuses everything, paired with the deny-all directory.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllCredentialDecryptor;
impl ProviderCredentialDecryptor for DenyAllCredentialDecryptor {
    fn decrypt<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
    ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
        Box::pin(async move {
            Err(CredentialResolveError::NotFound {
                requested: Some(binding.id),
            })
        })
    }
}

/// A decrypted provider key.
///
/// No `Clone`, no `Debug`, no `Display`, no `Serialize`, no `Deref<Target =
/// str>`. The plaintext leaves this type through exactly one method,
/// [`ProviderApiKey::sensitive_header`], which returns a `HeaderValue` already
/// marked sensitive. `Drop` zeroizes.
pub struct ProviderApiKey(Zeroizing<String>);

impl ProviderApiKey {
    /// Wraps decrypted material.
    ///
    /// Called only by a [`ProviderCredentialDecryptor`] implementation.
    #[must_use]
    pub fn new(plaintext: String) -> Self {
        Self(Zeroizing::new(plaintext))
    }

    /// Builds the one header the transport attaches, already marked sensitive
    /// so `reqwest` and `tracing` both redact it.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialResolveError::DecryptFailed`] when the material
    /// contains a byte no HTTP header value may carry, which means the stored
    /// ciphertext did not decrypt to a key.
    pub fn sensitive_header(
        &self,
        scheme: AuthScheme,
    ) -> Result<(reqwest::header::HeaderName, reqwest::header::HeaderValue), CredentialResolveError>
    {
        let rendered = match scheme {
            AuthScheme::BearerAuthorization => Zeroizing::new(format!("Bearer {}", *self.0)),
            AuthScheme::AnthropicApiKey { .. } | AuthScheme::GoogleApiKeyHeader => {
                Zeroizing::new(self.0.to_string())
            }
        };
        let name = reqwest::header::HeaderName::from_static(scheme.header_name());
        let mut value = reqwest::header::HeaderValue::from_str(&rendered)
            .map_err(|_| CredentialResolveError::DecryptFailed)?;
        value.set_sensitive(true);
        Ok((name, value))
    }

    /// The plaintext, for the redactor only.
    ///
    /// Crate-private: the redactor needs the exact bytes to remove them from a
    /// provider-echoed error body, and nothing else in the crate may look.
    pub(crate) fn expose_for_redaction(&self) -> &str {
        &self.0
    }
}

/// The cache key. Every component participates, so a rotation or a generation
/// bump can never hit a stale entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CredentialCacheKey {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Which binding.
    pub binding: ProviderCredentialId,
    /// Which revision.
    pub revision: CredentialRevision,
    /// Which generation.
    pub generation: SourceGeneration,
}

impl From<&ProviderCredentialBinding> for CredentialCacheKey {
    fn from(binding: &ProviderCredentialBinding) -> Self {
        Self {
            workspace: binding.workspace,
            binding: binding.id,
            revision: binding.revision,
            generation: binding.generation,
        }
    }
}

/// How long a decrypted key may be reused.
pub const CACHE_TTL: Duration = Duration::from_mins(1);
/// How many decrypted keys may be held at once.
pub const CACHE_CAPACITY: usize = 256;

struct CacheEntry {
    key: ProviderApiKey,
    inserted_at: Instant,
}

/// A bounded, expiring cache of decrypted keys.
///
/// Decryption happens immediately before dispatch and the plaintext exists only
/// inside an entry and inside the sensitive `HeaderValue`. Revocation calls
/// [`CredentialCache::invalidate`], which drops — and therefore zeroizes —
/// every matching entry.
pub struct CredentialCache {
    entries: Mutex<HashMap<CredentialCacheKey, CacheEntry>>,
    ttl: Duration,
    capacity: usize,
}

impl core::fmt::Debug for CredentialCache {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Deliberately opaque: a cache whose `Debug` listed its keys would put
        // the workspace/binding graph into every log line that formats it.
        formatter.write_str("CredentialCache { .. }")
    }
}

impl Default for CredentialCache {
    fn default() -> Self {
        Self::new(CACHE_CAPACITY, CACHE_TTL)
    }
}

impl CredentialCache {
    /// Builds a cache with an explicit capacity and TTL.
    #[must_use]
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            capacity,
        }
    }

    /// Decrypts a binding, reusing a fresh entry where one exists.
    ///
    /// The returned key is a fresh decryption rather than a handle into the
    /// cache, so a concurrent [`CredentialCache::invalidate`] can never leave a
    /// caller holding a reference into freed material.
    ///
    /// # Errors
    ///
    /// Propagates the decryptor's own [`CredentialResolveError`].
    pub async fn decrypt(
        &self,
        binding: &ProviderCredentialBinding,
        decryptor: &dyn ProviderCredentialDecryptor,
    ) -> Result<ProviderApiKey, CredentialResolveError> {
        let key = CredentialCacheKey::from(binding);
        if let Some(cached) = self.take_fresh(&key) {
            return Ok(cached);
        }
        let decrypted = decryptor.decrypt(binding).await?;
        self.insert(
            key,
            ProviderApiKey::new(decrypted.expose_for_redaction().to_owned()),
        );
        Ok(decrypted)
    }

    fn take_fresh(&self, key: &CredentialCacheKey) -> Option<ProviderApiKey> {
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.get(key)?;
        if entry.inserted_at.elapsed() >= self.ttl {
            entries.remove(key);
            return None;
        }
        Some(ProviderApiKey::new(
            entry.key.expose_for_redaction().to_owned(),
        ))
    }

    fn insert(&self, key: CredentialCacheKey, value: ProviderApiKey) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        entries.retain(|_, entry| entry.inserted_at.elapsed() < self.ttl);
        if entries.len() >= self.capacity {
            // Evict the oldest. Dropping the entry zeroizes it.
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| *key)
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            key,
            CacheEntry {
                key: value,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Drops every entry for a binding. Returns how many were removed.
    pub fn invalidate(&self, workspace: WorkspaceId, binding: ProviderCredentialId) -> usize {
        let Ok(mut entries) = self.entries.lock() else {
            return 0;
        };
        let before = entries.len();
        entries.retain(|key, _| !(key.workspace == workspace && key.binding == binding));
        before - entries.len()
    }

    /// Drops every entry for a workspace.
    pub fn invalidate_workspace(&self, workspace: WorkspaceId) -> usize {
        let Ok(mut entries) = self.entries.lock() else {
            return 0;
        };
        let before = entries.len();
        entries.retain(|key, _| key.workspace != workspace);
        before - entries.len()
    }

    /// How many entries are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().map_or(0, |entries| entries.len())
    }

    /// Whether the cache holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Resolves a binding and enforces the affinity and revocation rules.
///
/// # Errors
///
/// Returns [`CredentialResolveError`] for a missing, deleted, revoked or
/// provider-mismatched binding. Every arm happens before the send gate is
/// consumed, so every one is `DispatchProof::NotSent`.
pub async fn resolve(
    directory: &dyn ProviderCredentialDirectory,
    workspace: WorkspaceId,
    provider: ProviderId,
    requested: Option<ProviderCredentialId>,
    pin: Option<&SessionCredentialPin>,
) -> Result<ProviderCredentialBinding, CredentialResolveError> {
    let binding = directory.resolve(workspace, provider, requested).await?;

    if binding.provider != provider {
        return Err(CredentialResolveError::ProviderMismatch {
            binding: binding.provider,
            requested: provider,
        });
    }
    match binding.state {
        BindingState::Deleted => return Err(CredentialResolveError::Deleted),
        BindingState::Revoked => {
            return Err(CredentialResolveError::Revoked {
                admitted: pin.map_or(binding.revocation_epoch, |pin| pin.epoch_at_admission),
                current: binding.revocation_epoch,
            });
        }
        BindingState::Ready => {}
    }
    if let Some(pin) = pin
        && binding.revocation_epoch > pin.epoch_at_admission
    {
        return Err(CredentialResolveError::Revoked {
            admitted: pin.epoch_at_admission,
            current: binding.revocation_epoch,
        });
    }
    Ok(binding)
}

#[cfg(test)]
mod tests {
    use aex_wire::CanonicalJson;
    use aex_wire::ids::{PrefixedId, ProviderCredentialId, WorkspaceId};
    use aex_wire::provider::ProviderId;

    use super::{
        BindingState, CredentialBindingRef, CredentialCache, CredentialResolveError,
        CredentialRevision, DenyAllCredentialDecryptor, DenyAllCredentialDirectory, ProviderApiKey,
        ProviderCredentialBinding, ProviderCredentialDecryptor, ProviderCredentialDirectory,
        SessionCredentialPin, resolve,
    };
    use crate::transport::AuthScheme;
    use crate::wire_pending::{
        BoxFuture, CiphertextRef, EncryptionContext, RevocationEpoch, SourceGeneration,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [1; 10]))
    }

    fn binding_id(seed: u8) -> ProviderCredentialId {
        ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(2, [seed; 10]))
    }

    fn binding(provider: ProviderId, state: BindingState, epoch: u64) -> ProviderCredentialBinding {
        ProviderCredentialBinding {
            id: binding_id(7),
            workspace: workspace(),
            provider,
            revision: CredentialRevision(1),
            generation: SourceGeneration(1),
            revocation_epoch: RevocationEpoch(epoch),
            is_default: true,
            state,
            ciphertext: CiphertextRef(
                aex_model_catalog::primitives::BoundedString::new("kms://ref").expect("ref"),
            ),
            context: EncryptionContext {
                workspace: workspace(),
                extra: CanonicalJson::parse("{}").expect("json"),
            },
        }
    }

    struct Fixed(ProviderCredentialBinding);
    impl ProviderCredentialDirectory for Fixed {
        fn resolve(
            &self,
            _workspace: WorkspaceId,
            _provider: ProviderId,
            _id: Option<ProviderCredentialId>,
        ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>> {
            let binding = self.0.clone();
            Box::pin(async move { Ok(binding) })
        }

        fn current_epoch(
            &self,
            _workspace: WorkspaceId,
            _id: ProviderCredentialId,
        ) -> BoxFuture<'_, Result<RevocationEpoch, CredentialResolveError>> {
            let epoch = self.0.revocation_epoch;
            Box::pin(async move { Ok(epoch) })
        }
    }

    struct Constant(&'static str);
    impl ProviderCredentialDecryptor for Constant {
        fn decrypt<'a>(
            &'a self,
            _binding: &'a ProviderCredentialBinding,
        ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
            let text = self.0.to_owned();
            Box::pin(async move { Ok(ProviderApiKey::new(text)) })
        }
    }

    #[tokio::test]
    async fn the_placeholder_directory_refuses_everything() {
        let directory = DenyAllCredentialDirectory;
        let error = resolve(&directory, workspace(), ProviderId::Anthropic, None, None)
            .await
            .expect_err("the placeholder must refuse");
        assert!(matches!(error, CredentialResolveError::NotFound { .. }));
        assert_eq!(
            error.error_code(),
            aex_wire::ErrorCode::ProviderCredentialNotFound
        );
    }

    #[tokio::test]
    async fn the_placeholder_decryptor_refuses_everything() {
        let decryptor = DenyAllCredentialDecryptor;
        // `ProviderApiKey` has no `Debug`, so the success arm cannot be
        // formatted into a panic message — which is the point.
        match decryptor
            .decrypt(&binding(ProviderId::Openai, BindingState::Ready, 0))
            .await
        {
            Ok(_) => panic!("the placeholder must refuse"),
            Err(error) => assert!(matches!(error, CredentialResolveError::NotFound { .. })),
        }
    }

    #[tokio::test]
    async fn a_provider_mismatch_is_typed_never_a_silent_substitution() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Ready, 0));
        let error = resolve(&directory, workspace(), ProviderId::Anthropic, None, None)
            .await
            .expect_err("a mismatch must fail");
        assert_eq!(
            error,
            CredentialResolveError::ProviderMismatch {
                binding: ProviderId::Openai,
                requested: ProviderId::Anthropic,
            }
        );
    }

    #[tokio::test]
    async fn a_revoked_binding_fails() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Revoked, 3));
        let error = resolve(&directory, workspace(), ProviderId::Openai, None, None)
            .await
            .expect_err("a revoked binding must fail");
        assert!(matches!(error, CredentialResolveError::Revoked { .. }));
        assert_eq!(
            error.error_code(),
            aex_wire::ErrorCode::ProviderCredentialRevoked
        );
    }

    #[tokio::test]
    async fn a_deleted_binding_fails() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Deleted, 0));
        let error = resolve(&directory, workspace(), ProviderId::Openai, None, None)
            .await
            .expect_err("a deleted binding must fail");
        assert_eq!(error, CredentialResolveError::Deleted);
    }

    #[tokio::test]
    async fn an_epoch_bump_overrides_an_existing_session_pin() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Ready, 5));
        let pin = SessionCredentialPin {
            binding: binding_id(7),
            revision: CredentialRevision(1),
            generation: SourceGeneration(1),
            epoch_at_admission: RevocationEpoch(4),
        };
        let error = resolve(
            &directory,
            workspace(),
            ProviderId::Openai,
            None,
            Some(&pin),
        )
        .await
        .expect_err("a bumped epoch must override the pin");
        assert_eq!(
            error,
            CredentialResolveError::Revoked {
                admitted: RevocationEpoch(4),
                current: RevocationEpoch(5),
            }
        );
    }

    #[tokio::test]
    async fn a_matching_epoch_still_resolves() {
        let directory = Fixed(binding(ProviderId::Openai, BindingState::Ready, 4));
        let pin = SessionCredentialPin {
            binding: binding_id(7),
            revision: CredentialRevision(1),
            generation: SourceGeneration(1),
            epoch_at_admission: RevocationEpoch(4),
        };
        resolve(
            &directory,
            workspace(),
            ProviderId::Openai,
            None,
            Some(&pin),
        )
        .await
        .expect("an unrevoked pin resolves");
    }

    #[test]
    fn the_sensitive_header_is_marked_sensitive_and_hides_in_debug() {
        let key = ProviderApiKey::new("sk-secret-value-0123456789".to_owned());
        let (name, value) = key
            .sensitive_header(AuthScheme::BearerAuthorization)
            .expect("header");
        assert_eq!(name.as_str(), "authorization");
        assert!(value.is_sensitive());
        assert!(!format!("{value:?}").contains("sk-secret"));
    }

    #[test]
    fn anthropic_and_google_send_the_bare_key_not_a_bearer_prefix() {
        let key = ProviderApiKey::new("sk-ant-0123456789".to_owned());
        let (name, _) = key
            .sensitive_header(AuthScheme::AnthropicApiKey {
                version: "2023-06-01",
            })
            .expect("header");
        assert_eq!(name.as_str(), "x-api-key");
        let (name, _) = key
            .sensitive_header(AuthScheme::GoogleApiKeyHeader)
            .expect("header");
        assert_eq!(name.as_str(), "x-goog-api-key");
    }

    #[test]
    fn a_key_with_an_illegal_header_byte_fails_rather_than_being_sanitised() {
        let key = ProviderApiKey::new("sk-\nInjected: header".to_owned());
        assert_eq!(
            key.sensitive_header(AuthScheme::BearerAuthorization)
                .expect_err("a newline cannot enter a header"),
            CredentialResolveError::DecryptFailed
        );
    }

    #[tokio::test]
    async fn the_cache_reuses_a_fresh_entry_and_expires_a_stale_one() {
        let cache = CredentialCache::new(4, core::time::Duration::from_millis(50));
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        cache
            .decrypt(&binding, &Constant("sk-one"))
            .await
            .expect("first decrypt");
        assert_eq!(cache.len(), 1);
        let reused = cache
            .decrypt(&binding, &Constant("sk-two"))
            .await
            .expect("cached");
        assert_eq!(reused.expose_for_redaction(), "sk-one");

        tokio::time::sleep(core::time::Duration::from_millis(60)).await;
        let refreshed = cache
            .decrypt(&binding, &Constant("sk-two"))
            .await
            .expect("expired then decrypted again");
        assert_eq!(refreshed.expose_for_redaction(), "sk-two");
    }

    #[tokio::test]
    async fn a_rotation_never_hits_a_stale_entry() {
        let cache = CredentialCache::default();
        let first = binding(ProviderId::Openai, BindingState::Ready, 0);
        let mut rotated = first.clone();
        rotated.revision = CredentialRevision(2);

        cache
            .decrypt(&first, &Constant("sk-old"))
            .await
            .expect("first");
        let after = cache
            .decrypt(&rotated, &Constant("sk-new"))
            .await
            .expect("rotated");
        assert_eq!(after.expose_for_redaction(), "sk-new");
        assert_eq!(cache.len(), 2);
    }

    #[tokio::test]
    async fn invalidate_drops_exactly_the_matching_entries() {
        let cache = CredentialCache::default();
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        cache
            .decrypt(&binding, &Constant("sk-one"))
            .await
            .expect("decrypt");
        assert_eq!(cache.invalidate(workspace(), binding_id(9)), 0);
        assert_eq!(cache.invalidate(workspace(), binding.id), 1);
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn the_cache_stays_inside_its_capacity() {
        let cache = CredentialCache::new(2, core::time::Duration::from_mins(1));
        for seed in 0..8u8 {
            let mut entry = binding(ProviderId::Openai, BindingState::Ready, 0);
            entry.revision = CredentialRevision(u64::from(seed));
            cache
                .decrypt(&entry, &Constant("sk-value"))
                .await
                .expect("decrypt");
        }
        assert!(cache.len() <= 2, "cache grew to {}", cache.len());
    }

    #[test]
    fn the_cache_debug_rendering_names_no_binding() {
        let cache = CredentialCache::default();
        assert_eq!(format!("{cache:?}"), "CredentialCache { .. }");
    }

    #[test]
    fn a_binding_ref_carries_only_non_secret_identity() {
        let binding = binding(ProviderId::Openai, BindingState::Ready, 0);
        let reference = CredentialBindingRef::from(&binding);
        let rendered = serde_json::to_string(&reference).expect("serialize");
        assert!(!rendered.contains("kms://"), "{rendered}");
        assert!(rendered.contains("pcr_"), "{rendered}");
    }
}
