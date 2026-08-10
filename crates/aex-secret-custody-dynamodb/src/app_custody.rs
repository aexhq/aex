//! The production `SecretCustodyReader`.
//!
//! `aex-secret-custody-dynamodb` was already bound into `session-stream-api`,
//! and no type implemented `aex_session_app::ports::SecretCustodyReader`, so
//! `session_credential_rebind` — whose domain logic has been complete for some
//! time — had no way to read the two things it needs.
//!
//! Two shape gaps stood between the store and the port, and neither is closed
//! by widening a type.
//!
//! **A metadata row can never carry sealed bytes.** That is the whole point of
//! splitting `SecretMetadata` from `StoredGeneration`: the list path has nowhere
//! to leak a ciphertext. But `WorkspaceSecret` — the value the custody domain
//! admits — needs the ciphertext, because `entries_from` refuses a selection
//! whose ciphertext is absent. So this adapter reads both rows per name and
//! joins them, and it is the only place in the tree where that join happens.
//!
//! **The encryption context is not stored, only its digest.** Rather than
//! inventing a context or carrying the digest around in a field that does not
//! exist, this rebuilds the context from the identifiers it is bound to and
//! **verifies** the rebuild against the stored digest. A mismatch is `Corrupt`
//! and the read fails. That turns "the context I would decrypt under" from an
//! assumption into a checked fact, at the cost of one hash per secret.

use aex_secret_domain::context::Plane;
use aex_secret_domain::{
    CustodyEntry, CustodyRevision, EncryptionContext, SecretName, SessionCustody, WorkspaceSecret,
};
use aex_session_app::ports::{PortError, SecretCustodyReader};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::ids::{OrganizationId, SessionId, WorkspaceId};
use aex_wire::types::Region;

use crate::codec::{CustodyHead, SecretMetadata, StoredGeneration};
use crate::store::{CustodyStore, SecretCustodyStore};

/// The largest custody set one read will reconstruct.
///
/// `SessionCredentialRebindRequest.secrets` is capped at 64 by the wire schema,
/// so a custody row can never legitimately hold more. The budget is above that
/// rather than equal to it, so a row that somehow exceeds the wire cap is a
/// typed refusal from the store and never a silently short entry list.
const CUSTODY_BUDGET: u32 = 128;

/// Reads secret custody for one verified request.
///
/// Bound to a tenant rather than taking one per call, because two of the three
/// facts an encryption context needs — the plane and the organization — are
/// request identity, not arguments the port carries.
#[derive(Debug, Clone)]
pub struct SessionCustodyReads {
    store: CustodyStore,
    plane: Plane,
    region: Region,
    organization: OrganizationId,
    workspace: WorkspaceId,
}

impl SessionCustodyReads {
    /// Binds the reader to the tenant the regional edge already authenticated.
    #[must_use]
    pub const fn new(
        store: CustodyStore,
        plane: Plane,
        region: Region,
        organization: OrganizationId,
        workspace: WorkspaceId,
    ) -> Self {
        Self {
            store,
            plane,
            region,
            organization,
            workspace,
        }
    }

    /// Refuses a read for a workspace this request was not authorized for.
    ///
    /// Tenant isolation is correctness here, not a permission check: answering
    /// would bind one workspace's sealed bytes into another workspace's
    /// session.
    fn assert_tenant(&self, workspace: WorkspaceId) -> Result<(), PortError> {
        if workspace == self.workspace {
            return Ok(());
        }
        Err(PortError::Corrupt {
            kind: "workspace secret",
            reason: "the command names a workspace the request was not authorized for",
        })
    }

    /// The context one sealed generation is bound to, verified against the
    /// digest stored beside it.
    fn verified_context(
        &self,
        metadata: &SecretMetadata,
        stored: &StoredGeneration,
    ) -> Result<EncryptionContext, PortError> {
        let context = EncryptionContext {
            plane: self.plane,
            region: self.region,
            organization: self.organization,
            workspace: metadata.workspace,
            name: metadata.name.clone(),
            generation: stored.generation,
            // A workspace secret's own ciphertext is not session-scoped; only a
            // custody entry's is, and that one is sealed by the custody writer
            // under its own revision.
            custody_revision: None,
        };
        if context.digest() != stored.context_digest {
            return Err(PortError::Corrupt {
                kind: "workspace secret",
                reason: "the stored context digest does not match the context this request would \
                         decrypt under",
            });
        }
        Ok(context)
    }
}

#[async_trait::async_trait]
impl SecretCustodyReader for SessionCustodyReads {
    async fn read_secrets(
        &self,
        workspace: WorkspaceId,
        names: &[SecretName],
    ) -> Result<Vec<WorkspaceSecret>, PortError> {
        self.assert_tenant(workspace)?;
        let mut secrets = Vec::with_capacity(names.len());
        for name in names {
            let Some(metadata) = self
                .store
                .load_secret(workspace, name)
                .await
                .map_err(|error| port_error(&error, "workspace secret"))?
            else {
                // Absent, not an error. The caller compares what it asked for
                // against what came back and produces the typed `not_found`
                // itself, so a partial answer here can never read as complete.
                continue;
            };
            let Some(stored) = self
                .store
                .load_generation(workspace, name, metadata.generation)
                .await
                .map_err(|error| port_error(&error, "workspace secret"))?
            else {
                // The metadata names a generation whose sealed row is gone.
                // That is corruption, never "no such secret": reporting it as
                // absent would let a rebind quietly drop a credential the
                // caller explicitly asked to bind.
                return Err(PortError::Corrupt {
                    kind: "workspace secret",
                    reason: "the secret names a source generation that has no sealed row",
                });
            };
            let context = self.verified_context(&metadata, &stored)?;
            secrets.push(WorkspaceSecret {
                workspace: metadata.workspace,
                name: metadata.name,
                generation: metadata.generation,
                revision: metadata.revision,
                state: metadata.state,
                revocation_epoch: metadata.revocation_epoch,
                ciphertext: Some(stored.ciphertext),
                context,
                created_at: metadata.created_at,
                updated_at: metadata.updated_at,
                revoked_at: metadata.revoked_at,
            });
        }
        Ok(secrets)
    }

    async fn read_custody(&self, session: SessionId) -> Result<Option<SessionCustody>, PortError> {
        let Some(head) = self
            .store
            .load_custody(self.workspace, session)
            .await
            .map_err(|error| port_error(&error, "session custody"))?
        else {
            return Ok(None);
        };
        let entries = self.entries(session, head.revision).await?;
        Ok(Some(custody_of(&head, entries)))
    }
}

impl SessionCustodyReads {
    async fn entries(
        &self,
        session: SessionId,
        revision: CustodyRevision,
    ) -> Result<Vec<CustodyEntry>, PortError> {
        let budget = PageBudget::new(CUSTODY_BUDGET).map_err(|_| PortError::Corrupt {
            kind: "session custody",
            reason: "the custody read budget is not a legal page size",
        })?;
        let bindings = self
            .store
            .list_custody_bindings(self.workspace, session, revision, budget)
            .await
            .map_err(|error| port_error(&error, "session custody"))?;
        Ok(bindings.into_iter().map(|binding| binding.entry).collect())
    }
}

fn custody_of(head: &CustodyHead, entries: Vec<CustodyEntry>) -> SessionCustody {
    SessionCustody {
        session: head.session,
        workspace: head.workspace,
        revision: head.revision,
        owner_key_edge: head.owner_key_edge,
        state: head.state,
        entries,
        updated_at: head.updated_at,
    }
}

/// Maps a store failure onto the port vocabulary.
///
/// A decode failure and an over-budget listing are both `Corrupt`: the second
/// is the store refusing to answer a listing it cannot complete, and treating
/// it as merely unavailable would invite a retry that can never succeed.
fn port_error(error: &StoreError, kind: &'static str) -> PortError {
    match error {
        StoreError::Throttled { .. } | StoreError::Contended => PortError::Throttled { kind },
        StoreError::Corrupt(_) | StoreError::Invalid { .. } => PortError::Corrupt {
            kind,
            reason: "the stored custody record does not decode into the domain vocabulary, or the \
                     listing did not fit its budget",
        },
        _ => PortError::Unavailable { kind },
    }
}

#[cfg(test)]
mod tests {
    use aex_secret_domain::context::Plane;
    use aex_secret_domain::{EncryptionContext, SourceGeneration};
    use aex_wire::ids::{PrefixedId as _, Uuid7};
    use aex_wire::types::Region;

    #[test]
    fn a_context_rebuilt_from_the_identifiers_digests_to_the_stored_value() {
        // The whole reason the adapter may reconstruct a context rather than
        // store one: the digest is a total function of the identifiers, so a
        // rebuild either matches byte for byte or the read fails.
        let context = EncryptionContext {
            plane: Plane::Dev,
            region: Region::EuWest1,
            organization: aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            name: aex_secret_domain::SecretName::parse("alpha").expect("a legal name"),
            generation: SourceGeneration(4),
            custody_revision: None,
        };
        assert_eq!(context.digest(), context.clone().digest());

        let mut moved = context.clone();
        moved.generation = SourceGeneration(5);
        assert_ne!(
            context.digest(),
            moved.digest(),
            "a generation the caller did not ask for must not verify"
        );

        let mut session_scoped = context.clone();
        session_scoped.custody_revision = Some(aex_secret_domain::CustodyRevision(1));
        assert_ne!(
            context.digest(),
            session_scoped.digest(),
            "a workspace secret is not sealed under a custody revision, and the digest says so"
        );
    }
}
