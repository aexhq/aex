//! Envelope verification, in the one order that matters.
//!
//! Each step is cheaper than the next and a refusal must never reach a paid
//! operation. This mirrors the discipline the custody path already keeps, where
//! a context mismatch is caught before a KMS call is spent.
//!
//! 1. The request arrived on the only channel that exists — a private listener.
//!    Positional, and not checked here because there is nothing to check.
//! 2. The envelope's signature verifies against the key set; an unknown `kid`
//!    is refused.
//! 3. The audience is `tool_exec`, this plane, this region.
//! 4. It has not expired and its declared lifetime is at most thirty seconds.
//! 5. The principal is an agent session, and it carries no epoch subject.
//! 6. The account state is active. Anything else fails closed.
//! 7. The tenant is resolved **from the envelope**.
//!
//! Steps 2 through 6 are `aex_identity_domain::assertion::verify` plus the two
//! checks below it cannot make, because it does not know it is speaking for this
//! audience. Step 8 — the organization ceiling — is [`crate::spend`], and step 9
//! — the vendor credential — is [`crate::run`], in that order and no other.

use aex_control_domain::epoch::{Epoch, EpochSubjectKind};
use aex_identity_domain::assertion::{
    AssertedAccountState, Assertion, Audience, EpochProjection, KeyId, Plane, PrincipalKind,
    VerificationInputs, VerificationKeySet, VerifyError, agent_session_binding,
};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_internal_contracts::tool_exec::ToolExecRequest;
use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::Region;
use uuid::Uuid;

/// A projection that holds no revocation epoch, because it is never consulted.
///
/// A regional edge checks claimed epochs against a local projection it
/// replicates. This process replicates nothing, so a projection here would be a
/// second authority that could disagree with the first. Rather than build one
/// and let it answer optimistically, [`Admitter`] refuses any envelope that
/// carries an epoch subject at all: a Brain-minted assertion names none, so a
/// claim to the contrary is a claim this process cannot check and therefore must
/// not accept.
struct NoEpochSubjects;

impl EpochProjection for NoEpochSubjects {
    fn projected(&self, _kind: EpochSubjectKind, _id: Uuid) -> Epoch {
        Epoch::NEVER
    }
}

/// One verified call, with the tenant resolved from the envelope.
///
/// Constructed only by [`Admitter::admit`]. Its fields are the answer to "whose
/// call is this", and the request body contributed none of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedCall {
    organization: OrganizationId,
    workspace: WorkspaceId,
    session: Uuid,
}

impl AuthorizedCall {
    /// The organization whose ceiling this call is admitted against, and whose
    /// money it spends.
    #[must_use]
    pub const fn organization(&self) -> &OrganizationId {
        &self.organization
    }

    /// The workspace the call belongs to.
    #[must_use]
    pub const fn workspace(&self) -> &WorkspaceId {
        &self.workspace
    }

    /// The agent session the envelope named as its principal.
    #[must_use]
    pub const fn session(&self) -> Uuid {
        self.session
    }
}

/// Why a request was not admitted.
///
/// One outward-facing arm, [`crate::handler`] maps every one of these to
/// `not_authorized`. They are distinguished here so an operator can tell a
/// clock skew from a rotated key in telemetry, and merged there so a caller
/// cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionRefusal {
    /// The envelope was not exactly one canonical 323-byte assertion.
    #[error("the envelope is malformed")]
    Malformed,
    /// The envelope did not verify, or verified and claimed something wrong.
    #[error("the envelope did not verify: {0}")]
    Envelope(VerifyError),
    /// The principal was not an agent session.
    #[error("the envelope speaks for a principal this service does not accept")]
    WrongPrincipal,
    /// The envelope carried an epoch subject this process cannot check.
    #[error("the envelope claims a revocation epoch this service cannot check")]
    UncheckableEpoch,
    /// The account is not active.
    #[error("the account is not active")]
    AccountNotActive,
    /// The tenant identifiers in the envelope are not well formed.
    #[error("the envelope carries an identifier that is not a valid identity")]
    MalformedTenant,
}

/// The one thing that turns a request into a tenant.
pub struct Admitter {
    keys: VerificationKeySet,
    audience: Audience,
}

impl Admitter {
    /// Binds the admitter to the keys and the audience this deployment accepts.
    #[must_use]
    pub const fn new(keys: VerificationKeySet, plane: Plane, region: Region) -> Self {
        Self {
            keys,
            audience: Audience {
                plane,
                region,
                service: AssertionAudience::ToolExec,
            },
        }
    }

    /// Whether the key set holds anything at all.
    ///
    /// Readiness consults this: a process with no verification key would refuse
    /// every request, and reporting ready while doing so is the failure mode
    /// this answers.
    #[must_use]
    pub fn has_keys(&self) -> bool {
        !self.keys.is_empty()
    }

    /// Which keys this process accepts, for a composition assertion.
    #[must_use]
    pub fn accepts(&self, kid: KeyId, now_ms: u64) -> bool {
        self.keys.find(kid, now_ms).is_some()
    }

    /// Verifies `request` and resolves the tenant from its envelope.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionRefusal`] naming the first rule that failed. Nothing
    /// in this function reaches a store, a credential or the network.
    pub fn admit(
        &self,
        request: &ToolExecRequest,
        now_ms: u64,
    ) -> Result<AuthorizedCall, AdmissionRefusal> {
        let assertion =
            Assertion::from_issued(&request.assertion).map_err(|_| AdmissionRefusal::Malformed)?;

        // The binding is recomputed from the request this process is holding,
        // which is what makes a captured envelope useless for any other call.
        // Every input to it is a field the unverified request already carries,
        // because the request carries no tenant-shaped field to use instead.
        let expected =
            agent_session_binding(Uuid::from_bytes(*request.effect.get()), request.attempt);

        let claims = aex_identity_domain::assertion::verify(
            assertion.as_bytes(),
            &self.keys,
            &VerificationInputs {
                now_ms,
                audience: self.audience,
                credential_binding: &expected,
                projection: &NoEpochSubjects,
            },
        )
        .map_err(AdmissionRefusal::Envelope)?;

        // `verify` already refuses the mismatched pair, so this is the second of
        // two independent checks rather than the only one. It is here because a
        // verifier that depends on a rule living somewhere else is a verifier
        // that stops holding when the rule moves.
        if claims.principal_kind != PrincipalKind::AgentSession {
            return Err(AdmissionRefusal::WrongPrincipal);
        }
        if claims.epochs.used().next().is_some() {
            return Err(AdmissionRefusal::UncheckableEpoch);
        }
        if claims.account_state != AssertedAccountState::Active {
            return Err(AdmissionRefusal::AccountNotActive);
        }

        Ok(AuthorizedCall {
            organization: OrganizationId::from_uuid7(
                Uuid7::from_bytes(*claims.organization_id.as_bytes())
                    .map_err(|_| AdmissionRefusal::MalformedTenant)?,
            ),
            workspace: WorkspaceId::from_uuid7(
                Uuid7::from_bytes(*claims.workspace_id.as_bytes())
                    .map_err(|_| AdmissionRefusal::MalformedTenant)?,
            ),
            session: claims.principal_id,
        })
    }
}
