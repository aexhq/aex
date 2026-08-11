//! Dedicated BYOK provider-credential reads for session admission.
//!
//! Session creation selects one provider credential by its public identity. It
//! never reads generic workspace secrets or session custody, so this adapter
//! exposes only the stored binding facts the application port can decide on.

use aex_session_app::ports::{
    CredentialState as AppCredentialState, PortError, ProviderCredentialBinding,
    ProviderCredentialReader,
};
use aex_session_dynamodb::error::StoreError;
use aex_wire::ids::{ProviderCredentialId, WorkspaceId};

use crate::store::{CustodyStore, SecretCustodyStore};

/// Reads provider credentials for one authenticated workspace.
#[derive(Debug, Clone)]
pub struct ProviderCredentialReads {
    store: CustodyStore,
    workspace: WorkspaceId,
}

impl ProviderCredentialReads {
    /// Binds the reader to the workspace the regional edge authenticated.
    #[must_use]
    pub const fn new(store: CustodyStore, workspace: WorkspaceId) -> Self {
        Self { store, workspace }
    }

    fn assert_tenant(&self, workspace: WorkspaceId) -> Result<(), PortError> {
        if workspace == self.workspace {
            return Ok(());
        }
        Err(PortError::Corrupt {
            kind: "provider credential",
            reason: "the command names a workspace the request was not authorized for",
        })
    }
}

#[async_trait::async_trait]
impl ProviderCredentialReader for ProviderCredentialReads {
    async fn read_provider_credential(
        &self,
        workspace: WorkspaceId,
        credential: ProviderCredentialId,
    ) -> Result<Option<ProviderCredentialBinding>, PortError> {
        self.assert_tenant(workspace)?;
        self.store
            .load_provider_credential(workspace, credential)
            .await
            .map(|binding| binding.as_ref().map(provider_binding_of))
            .map_err(|error| port_error(&error))
    }
}

fn provider_binding_of(binding: &crate::codec::ProviderCredential) -> ProviderCredentialBinding {
    ProviderCredentialBinding {
        credential: binding.credential,
        provider: binding.provider,
        source_generation: binding.source_generation.0,
        revision: binding.revision,
        state: provider_state_of(binding.state),
    }
}

const fn provider_state_of(state: crate::codec::CredentialState) -> AppCredentialState {
    match state {
        crate::codec::CredentialState::Ready => AppCredentialState::Ready,
        crate::codec::CredentialState::Revoked => AppCredentialState::Revoked,
    }
}

fn port_error(error: &StoreError) -> PortError {
    match error {
        StoreError::Throttled { .. } | StoreError::Contended => PortError::Throttled {
            kind: "provider credential",
        },
        StoreError::Corrupt(_) | StoreError::Invalid { .. } => PortError::Corrupt {
            kind: "provider credential",
            reason: "the stored provider credential does not decode into the admission vocabulary",
        },
        _ => PortError::Unavailable {
            kind: "provider credential",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{AppCredentialState, provider_state_of};

    #[test]
    fn every_stored_provider_state_keeps_its_admission_meaning() {
        assert_eq!(
            provider_state_of(crate::codec::CredentialState::Ready),
            AppCredentialState::Ready
        );
        assert_eq!(
            provider_state_of(crate::codec::CredentialState::Revoked),
            AppCredentialState::Revoked
        );
    }
}
