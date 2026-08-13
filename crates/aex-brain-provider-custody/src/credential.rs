//! Provider-credential ports: the binding types, the resolve/decrypt port
//! traits, the decrypted key, and the deny-all placeholders.
//!
//! # Why these types live here
//!
//! `aex-brain-provider-custody` owns the direct BYOK dispatch boundary. The
//! binding directory itself belongs to `aex-secret-domain` plus
//! `aex-secret-custody-dynamodb` in the `regional-secret-custody` table
//! (OD-23): a `BYOK` key is a workspace secret, and giving it a second custody
//! home would create a second encryption authority. This module defines the two
//! ports dispatch needs, consumes them, and ships
//! [`DenyAllCredentialDirectory`] as the typed placeholder until the owning
//! stream lands.
//!
//! There is deliberately **no** plaintext-from-environment path. Not "disabled
//! by default" — absent, with a test asserting no such constructor exists.

use std::sync::Arc;

use aex_wire::ids::{OrganizationId, ProviderCredentialId, WorkspaceId};
use aex_wire::provider::ProviderId;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub use aex_brain_app::ports::BoxFuture;
pub use aex_brain_domain::wire_pending::SessionCredentialPin;
pub use aex_model_catalog::canonical::CredentialBindingRef;
pub use aex_secret_domain::{CiphertextRef, EncryptionContext, RevocationEpoch, SourceGeneration};

/// How a provider key is written into an outbound request.
///
/// A **tag**, not a value. There is no variant carrying key material and no
/// constructor taking any, which is what makes credential leakage a type error
/// rather than a review item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>` — `openai`, `deepseek`, `zai`,
    /// `moonshotai`.
    BearerAuthorization,
    /// `x-api-key: <key>` plus a pinned `anthropic-version` — `anthropic`.
    AnthropicApiKey {
        /// The pinned API version. Always `2023-06-01` (D-17).
        version: &'static str,
    },
    /// `x-goog-api-key: <key>` — `google`.
    ///
    /// The `?key=` query form is forbidden in this codebase: it would place the
    /// customer credential in a `URL` that reaches proxies, access logs and
    /// error strings (D-15).
    GoogleApiKeyHeader,
}

impl AuthScheme {
    /// The header name the key is written into.
    #[must_use]
    pub const fn header_name(self) -> &'static str {
        match self {
            Self::BearerAuthorization => "authorization",
            Self::AnthropicApiKey { .. } => "x-api-key",
            Self::GoogleApiKeyHeader => "x-goog-api-key",
        }
    }
}

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
    /// The sealed ciphertext fetched from the exact hidden source generation.
    /// Never plaintext.
    pub ciphertext: CiphertextRef,
    /// The encryption context a decrypt must present.
    pub context: EncryptionContext,
    /// Digest persisted beside the ciphertext and checked before any KMS call.
    pub context_digest: [u8; 32],
}

impl ProviderCredentialBinding {
    /// The non-secret identity recorded on a canonical provider receipt.
    #[must_use]
    pub const fn receipt_ref(&self) -> CredentialBindingRef {
        CredentialBindingRef {
            id: self.id,
            revision: self.revision.0,
            generation: self.generation.0,
        }
    }
}

/// Why one credential-authority operation failed before its current send.
///
/// The enclosing dispatch still preserves `ResponseStarted` when an earlier
/// in-call retry attempt already reached the provider.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialResolveError {
    /// The regional authority cannot yet mint a workspace-bound provider
    /// credential record safely.
    #[error(
        "provider credential registration authority is unavailable: no port exposes the wrapped branch key required by SecretCrypto::seal, and the workspace secret-name mint/collision contract is undecided"
    )]
    RegistrationAuthorityUnavailable,
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

/// The binding directory dispatch consumes.
///
/// Production composition resolves this from regional secret custody. The
/// registration write path remains separately fail closed until its secret-name
/// and wrapped branch-key contracts are decided.
pub trait ProviderCredentialDirectory: Send + Sync + 'static {
    /// Resolves a binding, either by explicit id or by the workspace default.
    fn resolve(
        &self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        provider: ProviderId,
        id: Option<ProviderCredentialId>,
    ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>>;

    /// Re-reads the mutable binding and secret fences immediately before send.
    fn revalidate<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
    ) -> BoxFuture<'a, Result<RevocationEpoch, CredentialResolveError>>;
}

/// The decryptor dispatch consumes.
///
/// A production implementation must consume the regional secret authority's
/// exact ciphertext generation and encryption context. No local alternate
/// custody path is permitted.
pub trait ProviderCredentialDecryptor: Send + Sync + 'static {
    /// Decrypts a binding's ciphertext immediately before dispatch.
    fn decrypt<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
        now: aex_wire::types::Timestamp,
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
        _organization: OrganizationId,
        _workspace: WorkspaceId,
        _provider: ProviderId,
        id: Option<ProviderCredentialId>,
    ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>> {
        let _ = id;
        Box::pin(async move { Err(CredentialResolveError::RegistrationAuthorityUnavailable) })
    }

    fn revalidate<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
    ) -> BoxFuture<'a, Result<RevocationEpoch, CredentialResolveError>> {
        let _ = binding;
        Box::pin(async move { Err(CredentialResolveError::RegistrationAuthorityUnavailable) })
    }
}

/// A decryptor that refuses everything, paired with the deny-all directory.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllCredentialDecryptor;
impl ProviderCredentialDecryptor for DenyAllCredentialDecryptor {
    fn decrypt<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
        _now: aex_wire::types::Timestamp,
    ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
        let _ = binding;
        Box::pin(async move { Err(CredentialResolveError::RegistrationAuthorityUnavailable) })
    }
}

/// A decrypted provider key.
///
/// No `Clone`, no `Debug`, no `Display`, no `Serialize`, no `Deref<Target =
/// str>`. The plaintext leaves this type through exactly one method,
/// [`ProviderApiKey::sensitive_header`], which returns a `HeaderValue` already
/// marked sensitive, or through the redaction-only [`ProviderApiKey::plaintext`]
/// accessor. A cache may share the allocation with an in-flight value; the
/// final owner drop zeroizes it.
pub struct ProviderApiKey(Arc<Zeroizing<String>>);

impl ProviderApiKey {
    /// Wraps decrypted material.
    ///
    /// Called only by a [`ProviderCredentialDecryptor`] implementation.
    #[must_use]
    pub fn new(plaintext: String) -> Self {
        Self(Arc::new(Zeroizing::new(plaintext)))
    }

    /// Wraps a shared allocation. Cache-internal: a key-cache crate shares one
    /// allocation across a flight's waiters instead of copying plaintext.
    #[doc(hidden)]
    #[must_use]
    pub fn from_shared(plaintext: Arc<Zeroizing<String>>) -> Self {
        Self(plaintext)
    }

    /// Hands the shared allocation to a cache and to a flight's waiters.
    ///
    /// Not a plaintext accessor: it moves the same reference-counted zeroizing
    /// allocation, and nothing can read the string through what it returns.
    #[doc(hidden)]
    #[must_use]
    pub fn into_shared(self) -> Arc<Zeroizing<String>> {
        self.0
    }

    /// The shared allocation itself, for a cache to prove two keys alias.
    #[doc(hidden)]
    #[must_use]
    pub const fn shared_allocation(&self) -> &Arc<Zeroizing<String>> {
        &self.0
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
            AuthScheme::BearerAuthorization => {
                Zeroizing::new(format!("Bearer {}", self.0.as_str()))
            }
            AuthScheme::AnthropicApiKey { .. } | AuthScheme::GoogleApiKeyHeader => {
                Zeroizing::new(self.0.as_str().to_owned())
            }
        };
        let name = reqwest::header::HeaderName::from_static(scheme.header_name());
        let mut value = reqwest::header::HeaderValue::from_str(&rendered)
            .map_err(|_| CredentialResolveError::DecryptFailed)?;
        value.set_sensitive(true);
        Ok((name, value))
    }

    /// The plaintext, for redaction only.
    ///
    /// The redactor needs the exact bytes to remove them from a
    /// provider-echoed error body. Hidden from docs on purpose: nothing else
    /// should reach for this.
    #[doc(hidden)]
    #[must_use]
    pub fn plaintext(&self) -> &str {
        self.0.as_str()
    }
}

#[cfg(test)]
mod tests {
    use aex_secret_domain::SecretName;
    use aex_secret_domain::context::Plane;
    use aex_wire::ids::{PrefixedId as _, ProviderCredentialId};
    use aex_wire::provider::ProviderId;
    use aex_wire::types::Region;

    use super::{
        AuthScheme, BindingState, CredentialResolveError, CredentialRevision,
        DenyAllCredentialDecryptor, ProviderApiKey, ProviderCredentialBinding,
        ProviderCredentialDecryptor,
    };
    use crate::credential::{CiphertextRef, RevocationEpoch, SourceGeneration};

    fn workspace() -> aex_wire::ids::WorkspaceId {
        aex_wire::ids::WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [1; 10]))
    }

    fn organization() -> aex_wire::ids::OrganizationId {
        aex_wire::ids::OrganizationId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
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
            ciphertext: CiphertextRef {
                key_generation: 1,
                wrapped_key: vec![1; 32],
                nonce: Vec::new(),
                ciphertext: vec![2; 32],
            },
            context: aex_secret_domain::EncryptionContext {
                plane: Plane::Dev,
                region: Region::EuWest1,
                organization: organization(),
                workspace: workspace(),
                name: SecretName::parse("provider-key").expect("name"),
                generation: SourceGeneration(1),
                custody_revision: None,
            },
            context_digest: aex_secret_domain::EncryptionContext {
                plane: Plane::Dev,
                region: Region::EuWest1,
                organization: organization(),
                workspace: workspace(),
                name: SecretName::parse("provider-key").expect("name"),
                generation: SourceGeneration(1),
                custody_revision: None,
            }
            .digest(),
        }
    }

    /// Whether a named type implements a named trait.
    ///
    /// Rust has no negative bound, so `ProviderApiKey: !Debug` cannot be
    /// asserted directly. Item resolution can assert it: the inherent constant
    /// is reachable only while the bound holds, and an unreachable inherent
    /// constant falls back to the blanket trait constant. Every use below
    /// asserts the same bound against a type that does implement it, which is
    /// what shows the probe still detects an implementation that exists rather
    /// than always answering no.
    macro_rules! implements {
        ($subject:ty: $($bound:tt)+) => {{
            /// Exactly one of the two constants is reachable for any one
            /// subject, and which one is reachable is the whole answer, so the
            /// other is dead by construction rather than by oversight.
            #[allow(dead_code, reason = "one of the two constants is dead by construction")]
            trait Absent {
                const IMPLEMENTS: bool = false;
            }
            impl<T: ?Sized> Absent for T {}

            #[allow(dead_code, reason = "a type-level question is never constructed")]
            struct Probe<T: ?Sized>(core::marker::PhantomData<T>);

            #[allow(dead_code, reason = "one of the two constants is dead by construction")]
            impl<T: ?Sized + $($bound)+> Probe<T> {
                const IMPLEMENTS: bool = true;
            }

            <Probe<$subject>>::IMPLEMENTS
        }};
    }

    fn now() -> aex_wire::types::Timestamp {
        aex_wire::types::Timestamp::from_unix_millis(1).expect("timestamp")
    }

    #[tokio::test]
    async fn the_placeholder_decryptor_refuses_everything() {
        let decryptor = DenyAllCredentialDecryptor;
        // `ProviderApiKey` has no `Debug`, so the success arm cannot be
        // formatted into a panic message — which is the point.
        match decryptor
            .decrypt(&binding(ProviderId::Openai, BindingState::Ready, 0), now())
            .await
        {
            Ok(_) => panic!("the placeholder must refuse"),
            Err(error) => assert_eq!(
                error,
                CredentialResolveError::RegistrationAuthorityUnavailable
            ),
        }
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

    #[test]
    fn the_decrypted_key_gained_no_render_or_copy_interface() {
        // Sharing one allocation between a flight's waiters must not have
        // widened the one way plaintext is allowed to leave this type.
        assert!(!implements!(ProviderApiKey: core::fmt::Debug));
        assert!(!implements!(ProviderApiKey: core::fmt::Display));
        assert!(!implements!(ProviderApiKey: Clone));
        assert!(!implements!(ProviderApiKey: serde::Serialize));
    }
}
