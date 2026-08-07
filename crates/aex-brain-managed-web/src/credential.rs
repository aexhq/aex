//! Session-scoped managed-search credential authority.
//!
//! The authority performs two strongly consistent point reads, then one atomic
//! authorization transaction, then one KMS-backed reveal. The transaction
//! revalidates both the mutable secret revocation fence and the exact custody
//! revision. No plaintext is cached or retained between dispatches.

use std::sync::Arc;

use aex_brain_app::ports::{BoxFuture, DispatchTicket};
use aex_secret_aws::{SealedSecret, SecretCrypto, SecretCryptoError};
use aex_secret_custody_dynamodb::{
    CallAuthorization, CustodyBinding, CustodyHead, CustodyStore, SecretCustodyStore,
};
use aex_secret_domain::context::{EncryptionContext, Plane};
use aex_secret_domain::custody::{CustodyRevision, CustodyState};
use aex_secret_domain::plaintext::SecretPlaintext;
use aex_secret_domain::secret::{SecretName, SecretRevision};
use aex_session_dynamodb::error::StoreError;
use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::{Region, Timestamp};

use crate::executor::{CredentialSourceError, WebSearchCredentialSource};
use crate::search::{WEB_SEARCH_SECRET_NAME, WebSearchCredential};

/// The exact custody operations needed on the managed-search hot path.
pub trait SessionCredentialCustody: Send + Sync + 'static {
    /// Reads the session custody head strongly consistently.
    fn load_head(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> BoxFuture<'_, Result<Option<CustodyHead>, CredentialSourceError>>;

    /// Reads one immutable binding by its complete key.
    fn load_binding<'a>(
        &'a self,
        workspace: WorkspaceId,
        session: SessionId,
        revision: CustodyRevision,
        name: &'a SecretName,
    ) -> BoxFuture<'a, Result<Option<CustodyBinding>, CredentialSourceError>>;

    /// Atomically grants one call after revalidating secret and custody fences.
    fn authorize<'a>(
        &'a self,
        authorization: &'a CallAuthorization,
        bound_source_revision: SecretRevision,
    ) -> BoxFuture<'a, Result<(), CredentialSourceError>>;
}

impl SessionCredentialCustody for CustodyStore {
    fn load_head(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> BoxFuture<'_, Result<Option<CustodyHead>, CredentialSourceError>> {
        Box::pin(async move {
            SecretCustodyStore::load_custody(self, workspace, session)
                .await
                .map_err(store_read_error)
        })
    }

    fn load_binding<'a>(
        &'a self,
        workspace: WorkspaceId,
        session: SessionId,
        revision: CustodyRevision,
        name: &'a SecretName,
    ) -> BoxFuture<'a, Result<Option<CustodyBinding>, CredentialSourceError>> {
        Box::pin(async move {
            self.load_custody_binding(workspace, session, revision, name)
                .await
                .map_err(store_read_error)
        })
    }

    fn authorize<'a>(
        &'a self,
        authorization: &'a CallAuthorization,
        bound_source_revision: SecretRevision,
    ) -> BoxFuture<'a, Result<(), CredentialSourceError>> {
        Box::pin(async move {
            self.authorize_managed_call(authorization, bound_source_revision)
                .await
                .map_err(|error| store_authorization_error(&error))
        })
    }
}

/// The one decrypt operation needed after durable authorization.
pub trait SessionCredentialDecryptor: Send + Sync + 'static {
    /// Reveals a session-scoped ciphertext under its exact context.
    fn reveal<'a>(
        &'a self,
        sealed: &'a SealedSecret,
        context: &'a EncryptionContext,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<SecretPlaintext, CredentialSourceError>>;
}

impl<T> SessionCredentialDecryptor for T
where
    T: SecretCrypto,
{
    fn reveal<'a>(
        &'a self,
        sealed: &'a SealedSecret,
        context: &'a EncryptionContext,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<SecretPlaintext, CredentialSourceError>> {
        Box::pin(async move {
            SecretCrypto::reveal(self, sealed, context, now)
                .await
                .map_err(|error| crypto_error(&error))
        })
    }
}

/// Stateless authority for the admitted `aex_web_search` session binding.
pub struct SessionCredentialAuthority<C, K> {
    custody: Arc<C>,
    crypto: Arc<K>,
    plane: Plane,
    region: Region,
}

impl<C, K> SessionCredentialAuthority<C, K> {
    /// Binds the regional custody and crypto adapters.
    #[must_use]
    pub const fn new(custody: Arc<C>, crypto: Arc<K>, plane: Plane, region: Region) -> Self {
        Self {
            custody,
            crypto,
            plane,
            region,
        }
    }
}

impl<C, K> core::fmt::Debug for SessionCredentialAuthority<C, K> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SessionCredentialAuthority")
            .field("plane", &self.plane)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl<C, K> WebSearchCredentialSource for SessionCredentialAuthority<C, K>
where
    C: SessionCredentialCustody,
    K: SessionCredentialDecryptor,
{
    fn resolve<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
    ) -> BoxFuture<'a, Result<WebSearchCredential, CredentialSourceError>> {
        Box::pin(async move {
            let session = wire_session(ticket)?;
            let name = SecretName::parse(WEB_SEARCH_SECRET_NAME)
                .map_err(|_| CredentialSourceError::Unavailable)?;
            let head = self
                .custody
                .load_head(ticket.workspace(), session)
                .await?
                .ok_or(CredentialSourceError::Missing)?;
            if head.workspace != ticket.workspace()
                || head.session != session
                || head.state != CustodyState::Active
            {
                return Err(CredentialSourceError::Missing);
            }
            let binding = self
                .custody
                .load_binding(ticket.workspace(), session, head.revision, &name)
                .await?
                .ok_or(CredentialSourceError::Missing)?;
            if binding.workspace != ticket.workspace()
                || binding.session != session
                || binding.revision != head.revision
                || binding.entry.name != name
            {
                return Err(CredentialSourceError::Missing);
            }

            let now = Timestamp::from_unix_millis(ticket.issued_at().millis())
                .map_err(|_| CredentialSourceError::Unavailable)?;
            let context = EncryptionContext {
                plane: self.plane,
                region: self.region,
                organization: ticket.organization(),
                workspace: ticket.workspace(),
                name: name.clone(),
                generation: binding.entry.source_generation,
                custody_revision: Some(head.revision),
            };
            if context.digest() != binding.context_digest {
                return Err(CredentialSourceError::Malformed);
            }

            let authorization = CallAuthorization {
                authorization_id: authorization_id(ticket),
                session,
                workspace: ticket.workspace(),
                name,
                custody_revision: head.revision,
                source_generation: binding.entry.source_generation,
                owner_key_edge: head.owner_key_edge,
                created_at: now,
            };
            self.custody
                .authorize(&authorization, binding.entry.source_revision)
                .await?;

            let sealed = SealedSecret {
                frame: binding.entry.ciphertext.ciphertext,
                context_digest: binding.context_digest,
                wrapped_branch_key: binding.entry.ciphertext.wrapped_key,
            };
            let plaintext = self.crypto.reveal(&sealed, &context, now).await?;
            WebSearchCredential::parse(plaintext.expose_for_encryption())
                .map_err(|_| CredentialSourceError::Malformed)
        })
    }
}

fn wire_session(ticket: &DispatchTicket) -> Result<SessionId, CredentialSourceError> {
    Uuid7::from_bytes(*ticket.key().session.0.as_bytes())
        .map(SessionId::from_uuid7)
        .map_err(|_| CredentialSourceError::Unavailable)
}

fn authorization_id(ticket: &DispatchTicket) -> String {
    format!("{}{:04x}", ticket.effect().to_hex(), ticket.attempt())
}

fn store_read_error(_error: StoreError) -> CredentialSourceError {
    CredentialSourceError::Unavailable
}

fn store_authorization_error(error: &StoreError) -> CredentialSourceError {
    if matches!(error, StoreError::PreconditionFailed { .. }) {
        CredentialSourceError::Missing
    } else {
        CredentialSourceError::Unavailable
    }
}

fn crypto_error(error: &SecretCryptoError) -> CredentialSourceError {
    match error {
        SecretCryptoError::KeyMaterial(_) => CredentialSourceError::Unavailable,
        SecretCryptoError::ContextMismatch
        | SecretCryptoError::Envelope(_)
        | SecretCryptoError::Plaintext => CredentialSourceError::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use aex_brain_app::ports::{CancelToken, FenceGuard};
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, EffectId, Fence, OwnerToken,
        SessionId as BrainSessionId, Timestamp as BrainTimestamp,
    };
    use aex_secret_domain::custody::{CustodyEntry, OwnerKeyEdgeId};
    use aex_secret_domain::revocation::RevocationEpoch;
    use aex_secret_domain::secret::{CiphertextRef, SourceGeneration};
    use aex_wire::ids::{OrganizationId, PrefixedId, Uuid7};

    use super::*;
    use crate::search::WebSearchProviderId;

    type HeadKey = (WorkspaceId, SessionId);
    type BindingKey = (WorkspaceId, SessionId, CustodyRevision);

    #[derive(Debug, Default)]
    struct FixtureCustody {
        heads: Mutex<BTreeMap<HeadKey, CustodyHead>>,
        bindings: Mutex<BTreeMap<BindingKey, CustodyBinding>>,
        authorizations: Mutex<Vec<(CallAuthorization, SecretRevision)>>,
        events: Arc<Mutex<Vec<&'static str>>>,
        deny_authorization: Mutex<bool>,
    }

    impl SessionCredentialCustody for FixtureCustody {
        fn load_head(
            &self,
            workspace: WorkspaceId,
            session: SessionId,
        ) -> BoxFuture<'_, Result<Option<CustodyHead>, CredentialSourceError>> {
            self.events.lock().expect("events").push("head");
            let value = self
                .heads
                .lock()
                .expect("heads")
                .get(&(workspace, session))
                .cloned();
            Box::pin(async move { Ok(value) })
        }

        fn load_binding<'a>(
            &'a self,
            workspace: WorkspaceId,
            session: SessionId,
            revision: CustodyRevision,
            _name: &'a SecretName,
        ) -> BoxFuture<'a, Result<Option<CustodyBinding>, CredentialSourceError>> {
            self.events.lock().expect("events").push("binding");
            let value = self
                .bindings
                .lock()
                .expect("bindings")
                .get(&(workspace, session, revision))
                .cloned();
            Box::pin(async move { Ok(value) })
        }

        fn authorize<'a>(
            &'a self,
            authorization: &'a CallAuthorization,
            bound_source_revision: SecretRevision,
        ) -> BoxFuture<'a, Result<(), CredentialSourceError>> {
            self.events.lock().expect("events").push("authorize");
            self.authorizations
                .lock()
                .expect("authorizations")
                .push((authorization.clone(), bound_source_revision));
            let denied = *self.deny_authorization.lock().expect("denial");
            Box::pin(async move {
                if denied {
                    Err(CredentialSourceError::Missing)
                } else {
                    Ok(())
                }
            })
        }
    }

    #[derive(Debug)]
    struct FixtureDecryptor {
        events: Arc<Mutex<Vec<&'static str>>>,
        contexts: Mutex<Vec<EncryptionContext>>,
    }

    impl SessionCredentialDecryptor for FixtureDecryptor {
        fn reveal<'a>(
            &'a self,
            sealed: &'a SealedSecret,
            context: &'a EncryptionContext,
            _now: Timestamp,
        ) -> BoxFuture<'a, Result<SecretPlaintext, CredentialSourceError>> {
            self.events.lock().expect("events").push("decrypt");
            self.contexts
                .lock()
                .expect("contexts")
                .push(context.clone());
            let provider = if sealed.frame.first() == Some(&1) {
                "brave"
            } else {
                "serper"
            };
            let bytes = format!(r#"{{"provider":"{provider}","apiKey":"test-key"}}"#).into_bytes();
            Box::pin(async move {
                SecretPlaintext::new(bytes).map_err(|_| CredentialSourceError::Malformed)
            })
        }
    }

    struct Fixture {
        custody: Arc<FixtureCustody>,
        decryptor: Arc<FixtureDecryptor>,
        authority: SessionCredentialAuthority<FixtureCustody, FixtureDecryptor>,
    }

    fn workspace(seed: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_800_000_000_000, [seed; 10]))
    }

    fn organization(seed: u8) -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1_800_000_000_000, [seed; 10]))
    }

    fn session(seed: u8) -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1_800_000_000_000, [seed; 10]))
    }

    fn ticket(seed: u8) -> DispatchTicket {
        let session = session(seed);
        let key = AgentKey::new(
            BrainSessionId(uuid::Uuid::from_bytes(*session.uuid7().as_bytes())),
            AgentId(uuid::Uuid::from_bytes(
                *Uuid7::compose(1_800_000_000_000, [seed.wrapping_add(1); 10]).as_bytes(),
            )),
        );
        let guard = FenceGuard::new(
            key,
            OwnerToken(uuid::Uuid::from_bytes(
                *Uuid7::compose(1_800_000_000_000, [seed.wrapping_add(2); 10]).as_bytes(),
            )),
            Fence(4),
            AgentRevision(5),
            None,
            CancelEpoch::ZERO,
            CancelToken::new(),
        );
        DispatchTicket::mint(
            &guard,
            workspace(seed),
            organization(seed),
            EffectId([seed; 16]),
            u16::from(seed),
            BrainTimestamp::from_millis(1_800_000_000_000),
        )
    }

    fn fixture() -> Fixture {
        let events = Arc::new(Mutex::new(Vec::new()));
        let custody = Arc::new(FixtureCustody {
            events: Arc::clone(&events),
            ..FixtureCustody::default()
        });
        let decryptor = Arc::new(FixtureDecryptor {
            events,
            contexts: Mutex::new(Vec::new()),
        });
        let authority = SessionCredentialAuthority::new(
            Arc::clone(&custody),
            Arc::clone(&decryptor),
            Plane::Prd,
            Region::ALL[0],
        );
        Fixture {
            custody,
            decryptor,
            authority,
        }
    }

    fn admit(fixture: &Fixture, seed: u8, provider_tag: u8) {
        let ticket = ticket(seed);
        let session = session(seed);
        let name = SecretName::parse(WEB_SEARCH_SECRET_NAME).expect("canonical name");
        let revision = CustodyRevision(u64::from(seed) + 1);
        let source_revision = SecretRevision(u64::from(seed) + 10);
        let context = EncryptionContext {
            plane: Plane::Prd,
            region: Region::ALL[0],
            organization: ticket.organization(),
            workspace: ticket.workspace(),
            name: name.clone(),
            generation: SourceGeneration(u64::from(seed) + 20),
            custody_revision: Some(revision),
        };
        let head = CustodyHead {
            session,
            workspace: ticket.workspace(),
            revision,
            owner_key_edge: OwnerKeyEdgeId(Uuid7::compose(
                1_800_000_000_000,
                [seed.wrapping_add(3); 10],
            )),
            state: CustodyState::Active,
            idle_epoch: 0,
            updated_at: Timestamp::from_unix_millis(1_800_000_000_000).expect("time"),
        };
        let binding = CustodyBinding {
            session,
            workspace: ticket.workspace(),
            revision,
            entry: CustodyEntry {
                name,
                source_generation: context.generation,
                source_revision,
                epoch_at_admission: RevocationEpoch::INITIAL,
                ciphertext: CiphertextRef {
                    key_generation: 1,
                    wrapped_key: vec![9; 32],
                    nonce: Vec::new(),
                    ciphertext: vec![provider_tag],
                },
            },
            context_digest: context.digest(),
        };
        fixture
            .custody
            .heads
            .lock()
            .expect("heads")
            .insert((ticket.workspace(), session), head);
        fixture
            .custody
            .bindings
            .lock()
            .expect("bindings")
            .insert((ticket.workspace(), session, revision), binding);
    }

    #[tokio::test]
    async fn authorization_is_durable_before_decrypt_and_carries_ticket_identity() {
        let fixture = fixture();
        admit(&fixture, 7, 1);

        let resolved = fixture
            .authority
            .resolve(&ticket(7))
            .await
            .expect("resolves");
        assert_eq!(resolved.provider(), WebSearchProviderId::Brave);

        let authorizations = fixture
            .custody
            .authorizations
            .lock()
            .expect("authorizations");
        assert_eq!(authorizations.len(), 1);
        let (authorization, source_revision) = &authorizations[0];
        assert_eq!(authorization.session, session(7));
        assert_eq!(authorization.workspace, workspace(7));
        assert_eq!(
            authorization.authorization_id,
            format!("{}{:04x}", EffectId([7; 16]), 7)
        );
        assert_eq!(*source_revision, SecretRevision(17));
        drop(authorizations);
        assert_eq!(
            fixture.custody.events.lock().expect("events").as_slice(),
            ["head", "binding", "authorize", "decrypt"]
        );
    }

    #[tokio::test]
    async fn a_revocation_race_denies_before_any_plaintext_is_revealed() {
        let fixture = fixture();
        admit(&fixture, 8, 1);
        *fixture.custody.deny_authorization.lock().expect("denial") = true;

        assert_eq!(
            fixture.authority.resolve(&ticket(8)).await.err(),
            Some(CredentialSourceError::Missing)
        );
        assert!(
            !fixture
                .decryptor
                .events
                .lock()
                .expect("events")
                .contains(&"decrypt")
        );
    }

    #[tokio::test]
    async fn a_foreign_binding_is_refused_before_authorization_or_decrypt() {
        let fixture = fixture();
        admit(&fixture, 9, 1);
        fixture
            .custody
            .bindings
            .lock()
            .expect("bindings")
            .values_mut()
            .next()
            .expect("binding")
            .workspace = workspace(99);

        assert_eq!(
            fixture.authority.resolve(&ticket(9)).await.err(),
            Some(CredentialSourceError::Missing)
        );
        assert!(
            fixture
                .custody
                .authorizations
                .lock()
                .expect("authorizations")
                .is_empty()
        );
        assert!(
            !fixture
                .decryptor
                .events
                .lock()
                .expect("events")
                .contains(&"decrypt")
        );
    }

    #[tokio::test]
    async fn concurrent_tenants_keep_contexts_and_credentials_isolated() {
        let fixture = fixture();
        for seed in 1_u8..=32 {
            admit(&fixture, seed, if seed % 2 == 0 { 1 } else { 2 });
        }
        let tickets: Vec<_> = (1_u8..=32).map(ticket).collect();
        let resolved = futures::future::join_all(
            tickets
                .iter()
                .map(|ticket| fixture.authority.resolve(ticket)),
        )
        .await;
        for (index, result) in resolved.into_iter().enumerate() {
            let seed = u8::try_from(index + 1).expect("bounded");
            let expected = if seed % 2 == 0 {
                WebSearchProviderId::Brave
            } else {
                WebSearchProviderId::Serper
            };
            assert_eq!(result.expect("resolves").provider(), expected);
        }
        let contexts = fixture.decryptor.contexts.lock().expect("contexts");
        assert_eq!(contexts.len(), 32);
        for context in contexts.iter() {
            assert_eq!(context.organization.uuid7(), context.workspace.uuid7());
            assert!(context.custody_revision.is_some());
        }
        assert_eq!(
            fixture
                .custody
                .authorizations
                .lock()
                .expect("authorizations")
                .len(),
            32
        );
    }
}
