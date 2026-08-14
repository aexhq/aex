//! Short-lived, body-bound `AgentSession` assertion verification.

use aex_control_domain::epoch::{Epoch, EpochSubjectKind};
use aex_identity_domain::assertion::{
    AssertedAccountState, Assertion, Audience, EpochProjection, Plane, PrincipalKind,
    VerificationInputs, VerificationKeySet, verify,
};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_wire::ids::{OrganizationId, PrefixedId as _, SessionId, WorkspaceId};
use axum::http::HeaderMap;

use crate::{AuthorizationScope, InternalAuthorizer};

/// Internal assertion header. It never accepts a customer credential.
pub const ASSERTION_HEADER: &str = "x-aex-tool-assertion";

struct EmptyProjection;

impl EpochProjection for EmptyProjection {
    fn projected(&self, _kind: EpochSubjectKind, _id: uuid::Uuid) -> Epoch {
        Epoch::NEVER
    }
}

/// Regional `ToolMux` assertion verifier.
pub struct AssertionAuthorizer {
    keys: VerificationKeySet,
    audience: Audience,
}

impl AssertionAuthorizer {
    /// Binds the release-projected verification keys and exact plane/region.
    #[must_use]
    pub const fn new(
        keys: VerificationKeySet,
        plane: Plane,
        region: aex_wire::types::Region,
    ) -> Self {
        Self {
            keys,
            audience: Audience {
                plane,
                region,
                service: AssertionAudience::ToolMux,
            },
        }
    }
}

impl InternalAuthorizer for AssertionAuthorizer {
    fn is_ready(&self) -> bool {
        !self.keys.is_empty()
    }

    fn authorize<'a>(
        &'a self,
        headers: &'a HeaderMap,
        scope: AuthorizationScope,
        binding: [u8; 32],
    ) -> aex_tool_mux::ToolMuxFuture<'a, Result<(), ()>> {
        Box::pin(async move {
            let text = headers
                .get(ASSERTION_HEADER)
                .and_then(|value| value.to_str().ok())
                .ok_or(())?;
            let assertion = Assertion::from_base64url(text).map_err(|_| ())?;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|value| u64::try_from(value.as_millis()).ok())
                .ok_or(())?;
            let claims = verify(
                assertion.as_bytes(),
                &self.keys,
                &VerificationInputs {
                    now_ms,
                    audience: self.audience,
                    credential_binding: &binding,
                    projection: &EmptyProjection,
                },
            )
            .map_err(|_| ())?;
            if claims.principal_kind != PrincipalKind::AgentSession
                || claims.account_state != AssertedAccountState::Active
                || claims.principal_id != uuid::Uuid::from_bytes(*scope.session.uuid7().as_bytes())
                || scope.organization.is_some_and(|organization| {
                    claims.organization_id
                        != uuid::Uuid::from_bytes(*organization.uuid7().as_bytes())
                })
                || scope.workspace.is_some_and(|workspace| {
                    claims.workspace_id != uuid::Uuid::from_bytes(*workspace.uuid7().as_bytes())
                })
            {
                return Err(());
            }
            Ok(())
        })
    }
}

/// Authorization scope for a request. Admission prepare has only a session;
/// execution also proves tenant ownership.
#[must_use]
pub const fn scope(
    session: SessionId,
    organization: Option<OrganizationId>,
    workspace: Option<WorkspaceId>,
) -> AuthorizationScope {
    AuthorizationScope {
        session,
        organization,
        workspace,
    }
}

#[cfg(test)]
mod tests {
    use aex_control_domain::scope::ScopeSet;
    use aex_identity_domain::assertion::{
        AssertionClaims, AssertionSigner as _, EpochSlots, KeyId, LocalSigner, VerificationKey,
    };
    use axum::http::{HeaderMap, HeaderValue};
    use zeroize::Zeroizing;

    use super::*;

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("fixture clock")
            .as_millis()
            .try_into()
            .expect("fixture clock fits")
    }

    #[tokio::test]
    async fn assertion_is_bound_to_body_session_tenant_and_audience() {
        let now = now_ms();
        let seed = Zeroizing::new([7_u8; 32]);
        let signer = LocalSigner::new(KeyId::new(uuid::Uuid::from_u128(1)), &seed);
        let keys = VerificationKeySet::new(vec![VerificationKey {
            kid: signer.kid(),
            public_key: signer.public_key(),
            not_after_ms: now + 60_000,
        }])
        .expect("fixture verification key");
        let authorizer =
            AssertionAuthorizer::new(keys, Plane::Dev, aex_wire::types::Region::EuWest1);
        let organization = OrganizationId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]));
        let workspace = WorkspaceId::from_uuid7(aex_wire::ids::Uuid7::compose(2, [2; 10]));
        let session = SessionId::from_uuid7(aex_wire::ids::Uuid7::compose(3, [3; 10]));
        let binding = [9_u8; 32];
        let claims = AssertionClaims {
            issued_at_ms: now,
            expires_at_ms: now + 30_000,
            audience: Audience {
                plane: Plane::Dev,
                region: aex_wire::types::Region::EuWest1,
                service: AssertionAudience::ToolMux,
            },
            principal_kind: PrincipalKind::AgentSession,
            principal_id: uuid::Uuid::from_bytes(*session.uuid7().as_bytes()),
            credential_binding: binding,
            organization_id: uuid::Uuid::from_bytes(*organization.uuid7().as_bytes()),
            workspace_id: uuid::Uuid::from_bytes(*workspace.uuid7().as_bytes()),
            workspace_region: aex_wire::types::Region::EuWest1,
            account_state: AssertedAccountState::Active,
            scopes: ScopeSet::EMPTY,
            epochs: EpochSlots::EMPTY,
        };
        let assertion = aex_identity_domain::assertion::issue(&signer, &claims)
            .expect("fixture assertion")
            .to_base64url();
        let mut headers = HeaderMap::new();
        headers.insert(
            ASSERTION_HEADER,
            HeaderValue::from_str(&assertion).expect("header"),
        );
        let exact = scope(session, Some(organization), Some(workspace));

        assert!(authorizer.authorize(&headers, exact, binding).await.is_ok());
        assert!(
            authorizer
                .authorize(&headers, exact, [8; 32])
                .await
                .is_err()
        );
        assert!(
            authorizer
                .authorize(
                    &headers,
                    scope(
                        SessionId::from_uuid7(aex_wire::ids::Uuid7::compose(4, [4; 10])),
                        Some(organization),
                        Some(workspace),
                    ),
                    binding,
                )
                .await
                .is_err()
        );
    }
}
