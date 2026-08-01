//! The IAM-invoked assertion issue: the one operation `central-authz` serves.
//!
//! `central-authz` is invoked, not routed. Its callers are the five regional
//! edges, and the caller's execution role *is* the authentication: a role
//! without `lambda:InvokeFunction` on this one function ARN cannot obtain an
//! assertion at all. There is therefore no path, no method and no HTTP surface
//! here — and, by RS-18, no router either. A health route nothing can reach is a
//! mounted route that cannot be served.
//!
//! # What crosses the boundary
//!
//! A request names a credential by `(id, presentedDigest)` and never carries
//! one. The stored verifier is `HMAC-SHA256(pepper, SHA-256(token))` — a MAC
//! over a *digest* — precisely so this is sufficient: the pepper is recomputed
//! over the transmitted digest and compared in constant time. The customer's
//! plaintext never leaves the region that received it, and this process cannot
//! log, trace or leak a secret it was never sent.
//!
//! # Refusal versus fault
//!
//! A credential that is unknown, mismatched, revoked, lapsed, region-mismatched
//! or attached to an inactive workspace answers
//! [`AssertionRefusal::NotAuthorized`] — one outcome, so an unauthenticated
//! caller cannot learn which of those it was. An unreadable account state
//! answers [`AssertionRefusal::AccountStateUnavailable`], because asserting a
//! state nobody could read is how a paused account keeps spending. Everything
//! else — an unreachable database, an unknown pepper version, an envelope that
//! will not build — is an [`IssueFault`] and is reported as an invocation
//! failure, so a caller never renders "your key is invalid" when the truth is
//! "we could not tell".

use std::collections::BTreeMap;

use aex_control_app::ports::{AuthorizationReader, StoreError};
use aex_identity_domain::assertion::{
    AssertionSigner, PrincipalKind, user_session_binding, workspace_key_binding,
};
use aex_identity_domain::credential::{Pepper, PepperVersion, PresentedDigest, Verifier, verify};
use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::assertion::{
    AssertionRefusal, AssertionResponse, ResolveSessionForWorkspace, ResolveWorkspaceKey,
};

use crate::{Config, IssueRefusal, issue_for_actor, issue_for_key};

/// The most peppers one ring holds.
///
/// A rotation needs two — the new pepper and the one existing verifiers were
/// computed under — and the bound is generous enough for a slow re-peppering.
/// It exists so an unknown-version lookup stays a scan over a fixed, tiny map.
pub const MAX_PEPPERS: usize = 8;

/// The largest secret document this module decodes.
pub const MAX_SECRET_BYTES: usize = 64 * 1_024;

/// Why start-up secret material was refused.
///
/// Every variant stops the process. There is no arm that degrades to an empty
/// ring or a generated pepper: an authority that cannot verify a credential must
/// not start, because the alternative is a process that refuses everything while
/// reporting ready, or one that accepts everything.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The secret could not be read.
    #[error("secret `{name}` could not be read: {reason}")]
    Unreadable {
        /// Which secret.
        name: String,
        /// What the service reported.
        reason: String,
    },
    /// The secret held no value.
    #[error("secret `{name}` holds no value")]
    Empty {
        /// Which secret.
        name: String,
    },
    /// The secret was larger than the decode bound.
    #[error("secret `{name}` is {found} bytes; at most {MAX_SECRET_BYTES} are decoded")]
    TooLarge {
        /// Which secret.
        name: String,
        /// How large it was.
        found: usize,
    },
    /// The document did not decode.
    #[error("secret `{name}` is not a valid {kind} document: {reason}")]
    Malformed {
        /// Which secret.
        name: String,
        /// Which document kind was expected.
        kind: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// The ring declared no pepper, which would verify nothing.
    #[error("secret `{name}` declares no pepper")]
    NoPeppers {
        /// Which secret.
        name: String,
    },
    /// The ring was past the bound.
    #[error("secret `{name}` declares {found} peppers; at most {MAX_PEPPERS} are held")]
    TooManyPeppers {
        /// Which secret.
        name: String,
        /// How many were declared.
        found: usize,
    },
    /// Two entries claimed the same version.
    #[error("secret `{name}` declares pepper version {version} twice")]
    DuplicateVersion {
        /// Which secret.
        name: String,
        /// The repeated version.
        version: u16,
    },
    /// Key material was not the exact length it must be.
    #[error("secret `{name}` declares unusable material")]
    Material {
        /// Which secret.
        name: String,
    },
    /// The private half does not match the public half the authority published.
    ///
    /// This is the signing self-test. A signer built from the wrong secret would
    /// produce envelopes no region can verify, and the first symptom would be a
    /// total regional outage rather than a start-up failure.
    #[error("the signing secret does not match the active public key")]
    SigningKeyMismatch,
    /// The authority named a secret this deployable did not declare.
    ///
    /// The capability manifest binds one secret id. Fetching whichever id a
    /// database row happens to hold would make a database write able to
    /// redirect this process at any secret its role can read.
    #[error("the active signing key names `{found}`, which this deployable did not declare")]
    UndeclaredSigningSecret {
        /// What the row named.
        found: String,
    },
}

/// The peppers stored verifiers may have been computed under.
///
/// A version this ring does not hold is a **fault**, not a refusal: the
/// credential is *unverifiable*, not invalid, and answering "not authorized"
/// would be a lie the caller would act on.
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

    /// The pepper a verifier at `version` was computed under.
    #[must_use]
    pub fn get(&self, version: PepperVersion) -> Option<&Pepper> {
        self.peppers.get(&version.get())
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PepperDocument {
    #[allow(
        dead_code,
        reason = "decoded so an unversioned or future document is refused rather than guessed at"
    )]
    schema_version: SchemaVersion,
    peppers: Vec<PepperEntry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PepperEntry {
    version: u16,
    material: String,
}

/// Decodes the pepper ring held at `AEX_CENTRAL_AUTHZ_PEPPER_SECRET_ID`.
///
/// ```json
/// { "schemaVersion": 1,
///   "peppers": [ { "version": 1, "material": "<43 chars unpadded base64url>" } ] }
/// ```
///
/// # Errors
///
/// Returns [`SecretError`] for an oversized, malformed, empty, over-long or
/// ambiguous document, or one declaring material that is not exactly 32 bytes.
/// Nothing here degrades: a document this function refuses stops the process.
pub fn parse_pepper_ring(name: &str, document: &str) -> Result<PepperRing, SecretError> {
    if document.len() > MAX_SECRET_BYTES {
        return Err(SecretError::TooLarge {
            name: name.to_owned(),
            found: document.len(),
        });
    }
    let decoded: PepperDocument =
        serde_json::from_str(document).map_err(|error| SecretError::Malformed {
            name: name.to_owned(),
            kind: "pepper ring",
            reason: error.to_string(),
        })?;
    if decoded.peppers.is_empty() {
        return Err(SecretError::NoPeppers {
            name: name.to_owned(),
        });
    }
    if decoded.peppers.len() > MAX_PEPPERS {
        return Err(SecretError::TooManyPeppers {
            name: name.to_owned(),
            found: decoded.peppers.len(),
        });
    }
    let mut peppers = BTreeMap::new();
    for entry in decoded.peppers {
        let material = decode_32(&entry.material).ok_or_else(|| SecretError::Material {
            name: name.to_owned(),
        })?;
        if peppers
            .insert(entry.version, Pepper::new(material))
            .is_some()
        {
            return Err(SecretError::DuplicateVersion {
                name: name.to_owned(),
                version: entry.version,
            });
        }
    }
    Ok(PepperRing { peppers })
}

/// Decodes the signing private key held at `AEX_CENTRAL_AUTHZ_SIGNING_SECRET_ID`.
///
/// The value is the 32 raw private-key bytes in canonical unpadded base64url and
/// nothing else. There is no document around it because there is nothing else to
/// say: which key it is comes from the authority, and whether it is the right one
/// is settled by the self-test rather than by a field beside it.
///
/// # Errors
///
/// Returns [`SecretError::Material`] for anything that is not exactly 32 bytes.
pub fn parse_signing_secret(name: &str, document: &str) -> Result<[u8; 32], SecretError> {
    if document.len() > MAX_SECRET_BYTES {
        return Err(SecretError::TooLarge {
            name: name.to_owned(),
            found: document.len(),
        });
    }
    decode_32(document.trim()).ok_or_else(|| SecretError::Material {
        name: name.to_owned(),
    })
}

fn decode_32(text: &str) -> Option<[u8; 32]> {
    aex_control_domain::codec::unbase64url(text)
        .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
}

/// Which operation an invocation asked for.
///
/// The two requests are structurally disjoint — one names an API key, the other
/// a browser session — so classification reads the payload rather than trusting
/// a discriminator a caller could set wrongly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Resolve a presented workspace API key.
    WorkspaceKey(Box<ResolveWorkspaceKey>),
    /// Resolve a presented browser session over one workspace.
    BrowserSession(Box<ResolveSessionForWorkspace>),
}

/// Why an invocation payload was not an assertion request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvocationError {
    /// The payload named neither operation.
    ///
    /// Deliberately not a refusal: an unrecognised payload means a caller and
    /// this binary disagree about the contract, and answering `not_authorized`
    /// would hide that behind a plausible authorization failure.
    #[error("the payload is neither `ResolveWorkspaceKey` nor `ResolveSessionForWorkspace`")]
    Unrecognised,
    /// The payload named an operation but did not decode as one.
    #[error("the payload is a malformed `{operation}`: {reason}")]
    Malformed {
        /// Which operation it looked like.
        operation: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

impl Invocation {
    /// Classifies one invocation payload.
    ///
    /// # Errors
    ///
    /// Returns [`InvocationError`] for a payload that names neither operation or
    /// that names one and does not decode as it.
    pub fn classify(payload: &serde_json::Value) -> Result<Self, InvocationError> {
        let Some(object) = payload.as_object() else {
            return Err(InvocationError::Unrecognised);
        };
        if object.contains_key("key") {
            return serde_json::from_value(payload.clone())
                .map(|request| Self::WorkspaceKey(Box::new(request)))
                .map_err(|error| InvocationError::Malformed {
                    operation: "ResolveWorkspaceKey",
                    reason: error.to_string(),
                });
        }
        if object.contains_key("browserSession") {
            return serde_json::from_value(payload.clone())
                .map(|request| Self::BrowserSession(Box::new(request)))
                .map_err(|error| InvocationError::Malformed {
                    operation: "ResolveSessionForWorkspace",
                    reason: error.to_string(),
                });
        }
        Err(InvocationError::Unrecognised)
    }
}

/// An internal failure that must never be reported as an authorization outcome.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IssueFault {
    /// The payload was not an assertion request.
    #[error(transparent)]
    Invocation(#[from] InvocationError),
    /// The authority could not be read.
    #[error("the authorization store could not answer: {0}")]
    Store(String),
    /// A stored verifier names a pepper version this process does not hold.
    ///
    /// The credential is unverifiable rather than invalid, so this is a fault.
    #[error("pepper version {version} is not held by this process")]
    UnknownPepper {
        /// The version the row named.
        version: u16,
    },
    /// The envelope refused the claims.
    #[error("the assertion envelope could not be built")]
    Malformed,
}

impl From<StoreError> for IssueFault {
    fn from(error: StoreError) -> Self {
        Self::Store(error.to_string())
    }
}

/// The composed issue path.
pub struct AssertionAuthority<R, S> {
    reader: R,
    signer: S,
    peppers: PepperRing,
    plane: aex_central_http::config::DeploymentPlane,
    ttl_ms: u64,
}

impl<R, S> AssertionAuthority<R, S>
where
    R: AuthorizationReader,
    S: AssertionSigner,
{
    /// Composes the authority over its resolved start-up inputs.
    pub const fn new(reader: R, signer: S, peppers: PepperRing, config: &Config) -> Self {
        Self {
            reader,
            signer,
            peppers,
            plane: config.plane,
            ttl_ms: config.assertion_ttl_ms,
        }
    }

    /// Answers one classified invocation.
    ///
    /// # Errors
    ///
    /// Returns [`IssueFault`] for an internal failure. An authorization decision
    /// is an `Ok(AssertionResponse::Refused)` and never an error.
    pub async fn answer(
        &self,
        invocation: &Invocation,
        now: time::OffsetDateTime,
    ) -> Result<AssertionResponse, IssueFault> {
        match invocation {
            Invocation::WorkspaceKey(request) => self.workspace_key(request, now).await,
            Invocation::BrowserSession(request) => self.browser_session(request, now).await,
        }
    }

    async fn workspace_key(
        &self,
        request: &ResolveWorkspaceKey,
        now: time::OffsetDateTime,
    ) -> Result<AssertionResponse, IssueFault> {
        let Some(key) = self.reader.resolve_workspace_key(raw(request.key)).await? else {
            return Ok(refused(AssertionRefusal::NotAuthorized));
        };
        // The key names the region it was minted for and the caller names the
        // region it is pinned to. Both are checked, here and at the edge;
        // neither is a single point of failure.
        if key.region != request.region {
            return Ok(refused(AssertionRefusal::NotAuthorized));
        }
        let digest = PresentedDigest::from_bytes(*request.presented_digest.get());
        if !self.credential_matches(key.pepper_version, &digest, key.verifier)? {
            return Ok(refused(AssertionRefusal::NotAuthorized));
        }
        let binding = workspace_key_binding(key.key_id, &digest);
        match issue_for_key(
            &self.signer,
            &key,
            request.audience,
            self.plane,
            binding,
            millis(now),
            self.ttl_ms,
        ) {
            Ok(assertion) => issued(&assertion),
            Err(refusal) => Ok(refused(outcome(refusal)?)),
        }
    }

    async fn browser_session(
        &self,
        request: &ResolveSessionForWorkspace,
        now: time::OffsetDateTime,
    ) -> Result<AssertionResponse, IssueFault> {
        let Some(actor) = self
            .reader
            .resolve_session_for_workspace(
                raw(request.browser_session),
                raw(request.workspace),
                now,
            )
            .await?
        else {
            return Ok(refused(AssertionRefusal::NotAuthorized));
        };
        // The caller names the person as well as the session. A session that
        // resolves to a different user is a caller bug or an attack, and either
        // way it must not mint an assertion for whoever the row happens to name.
        if actor.user_id != raw(request.user) || actor.region != request.region {
            return Ok(refused(AssertionRefusal::NotAuthorized));
        }
        let digest = PresentedDigest::from_bytes(*request.presented_digest.get());
        if !self.credential_matches(actor.pepper_version, &digest, actor.verifier)? {
            return Ok(refused(AssertionRefusal::NotAuthorized));
        }
        let binding = user_session_binding(actor.credential_id, &digest);
        match issue_for_actor(
            &self.signer,
            &actor,
            PrincipalKind::UserSession,
            request.audience,
            self.plane,
            binding,
            millis(now),
            self.ttl_ms,
        ) {
            Ok(assertion) => issued(&assertion),
            Err(refusal) => Ok(refused(outcome(refusal)?)),
        }
    }

    /// Recomputes the stored verifier over the transmitted digest.
    ///
    /// This is the whole reason a request may name a credential instead of
    /// carrying one, and it is constant-time: a timing oracle over a 32-byte MAC
    /// is a credential oracle.
    fn credential_matches(
        &self,
        pepper_version: u16,
        digest: &PresentedDigest,
        stored: [u8; 32],
    ) -> Result<bool, IssueFault> {
        let pepper = self.peppers.get(PepperVersion::new(pepper_version)).ok_or(
            IssueFault::UnknownPepper {
                version: pepper_version,
            },
        )?;
        Ok(verify(pepper, digest, &Verifier::from_bytes(stored)))
    }
}

const fn refused(reason: AssertionRefusal) -> AssertionResponse {
    AssertionResponse::Refused { reason }
}

fn issued(
    assertion: &aex_identity_domain::assertion::Assertion,
) -> Result<AssertionResponse, IssueFault> {
    assertion
        .to_issued()
        .map(|assertion| AssertionResponse::Issued { assertion })
        .map_err(|_| IssueFault::Malformed)
}

/// Projects an issue refusal onto the outcome vocabulary the caller acts on.
///
/// `Malformed` is deliberately not in that vocabulary: an envelope this process
/// could not build is its own defect, not the caller's credential.
const fn outcome(refusal: IssueRefusal) -> Result<AssertionRefusal, IssueFault> {
    match refusal {
        IssueRefusal::NotCurrent => Ok(AssertionRefusal::NotAuthorized),
        IssueRefusal::AccountStateUnavailable => Ok(AssertionRefusal::AccountStateUnavailable),
        IssueRefusal::Malformed => Err(IssueFault::Malformed),
    }
}

/// The raw payload of a prefixed identifier, as the authority statements bind it.
fn raw<I: aex_wire::ids::PrefixedId>(id: I) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*id.uuid7().as_bytes())
}

fn millis(now: time::OffsetDateTime) -> u64 {
    u64::try_from(now.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        Invocation, InvocationError, IssueFault, MAX_PEPPERS, PepperRing, SecretError,
        parse_pepper_ring, parse_signing_secret,
    };
    use crate::{AssertionAuthority, Composition, Config};
    use aex_central_http::capability::{AssertionSign, Declares};
    use aex_central_http::config::DeploymentPlane;
    use aex_control_app::ports::{
        AccountActorState, AuthorizationReader, CentralActorState, SigningKeyRecord, StoreError,
        WorkspaceKeyState,
    };
    use aex_control_domain::{
        AccountState, Epoch, OrgRole, OrganizationStatus, ScopeSet, WorkspaceStatus,
    };
    use aex_identity_domain::assertion::{
        Assertion, Audience, EpochProjection, KeyId, LocalSigner, Plane, PrincipalKind,
        VerificationInputs, VerificationKey, VerificationKeySet, VerifyError, user_session_binding,
        verify, workspace_key_binding,
    };
    use aex_identity_domain::credential::{Pepper, PresentedDigest, verifier};
    use aex_internal_contracts::SchemaVersion;
    use aex_internal_contracts::assertion::{
        AssertionAudience, AssertionRefusal, AssertionResponse, CredentialDigest,
        ResolveSessionForWorkspace, ResolveWorkspaceKey,
    };
    use aex_wire::ids::PrefixedId as _;
    use aex_wire::types::Region;
    use async_trait::async_trait;
    use uuid::Uuid;

    const NOW_MS: i64 = 1_767_225_600_000;
    const PEPPER: [u8; 32] = [11_u8; 32];
    const TOKEN: &str =
        "aex_wk_euw1_01kyw2qa4ne00r40r40m30e209_00000000000000000000000000000000000000000000";

    fn now() -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(NOW_MS) * 1_000_000)
            .expect("a representable instant")
    }

    fn digest() -> PresentedDigest {
        PresentedDigest::of(TOKEN)
    }

    fn key_uuid() -> Uuid {
        Uuid::from_u128(0x21)
    }

    fn session_uuid() -> Uuid {
        Uuid::from_u128(0x22)
    }

    fn user_uuid() -> Uuid {
        Uuid::from_u128(0x23)
    }

    fn workspace_uuid() -> Uuid {
        Uuid::from_u128(0x24)
    }

    fn ring() -> PepperRing {
        parse_pepper_ring("test", &pepper_document(&[(1, PEPPER)])).expect("a usable ring")
    }

    fn pepper_document(entries: &[(u16, [u8; 32])]) -> String {
        let peppers: Vec<String> = entries
            .iter()
            .map(|(version, material)| {
                format!(
                    r#"{{"version":{version},"material":"{}"}}"#,
                    aex_control_domain::codec::base64url(material)
                )
            })
            .collect();
        format!(r#"{{"schemaVersion":1,"peppers":[{}]}}"#, peppers.join(","))
    }

    fn signer() -> LocalSigner {
        crate::signer(
            &<Composition as Declares<AssertionSign>>::grant(),
            KeyId::new(Uuid::from_u128(9)),
            [7_u8; 32],
        )
    }

    fn config() -> Config {
        Config {
            plane: DeploymentPlane::Dev,
            region: Region::EuWest1,
            account_id: "000000000000".to_owned(),
            aurora_cluster_arn: "arn:aws:rds:eu-west-1:000000000000:cluster:aex".to_owned(),
            aurora_secret_arn: "arn:aws:secretsmanager:eu-west-1:000000000000:secret:a".to_owned(),
            signing_secret_id: "aex/dev/authz-signing/current".to_owned(),
            pepper_secret_id: "aex/dev/credential-pepper/ring".to_owned(),
            database: "aex".to_owned(),
            role: "aex_authz".to_owned(),
            assertion_ttl_ms: 30_000,
        }
    }

    fn key_state() -> WorkspaceKeyState {
        WorkspaceKeyState {
            key_id: key_uuid(),
            workspace_id: workspace_uuid(),
            organization_id: Uuid::from_u128(0x25),
            scopes: ScopeSet::ALL,
            verifier: *verifier(&Pepper::new(PEPPER), &digest()).as_bytes(),
            pepper_version: 1,
            key_revoked: false,
            region: Region::EuWest1,
            workspace_status: WorkspaceStatus::Active,
            organization_status: OrganizationStatus::Active,
            account_state: AccountState::Active,
            epoch_key: Epoch::new(1),
            epoch_workspace: Epoch::new(1),
            epoch_account: Epoch::new(1),
        }
    }

    fn actor_state() -> AccountActorState {
        AccountActorState {
            credential_id: session_uuid(),
            user_id: user_uuid(),
            membership_id: Uuid::from_u128(0x26),
            role: OrgRole::Owner,
            scopes: ScopeSet::ALL,
            verifier: *verifier(&Pepper::new(PEPPER), &digest()).as_bytes(),
            pepper_version: 1,
            credential_revoked: false,
            credential_expired: false,
            user_active: true,
            workspace_id: workspace_uuid(),
            organization_id: Uuid::from_u128(0x25),
            region: Region::EuWest1,
            account_state: AccountState::Active,
            epoch_user: Epoch::new(1),
            epoch_membership: Epoch::new(1),
            epoch_workspace: Epoch::new(1),
            epoch_account: Epoch::new(1),
        }
    }

    /// A reader that answers exactly what a case set it to, and records that the
    /// plaintext never reached it — it cannot, because no method takes one.
    #[derive(Default)]
    struct Reader {
        key: Option<WorkspaceKeyState>,
        actor: Option<AccountActorState>,
        fail: bool,
    }

    #[async_trait]
    impl AuthorizationReader for Reader {
        async fn resolve_workspace_key(
            &self,
            key_id: Uuid,
        ) -> Result<Option<WorkspaceKeyState>, StoreError> {
            if self.fail {
                return Err(StoreError::Unavailable);
            }
            Ok(self.key.clone().filter(|key| key.key_id == key_id))
        }

        async fn resolve_account_token_for_workspace(
            &self,
            _token_id: Uuid,
            _workspace_id: Uuid,
            _now: time::OffsetDateTime,
        ) -> Result<Option<AccountActorState>, StoreError> {
            Ok(None)
        }

        async fn resolve_session_for_workspace(
            &self,
            session_id: Uuid,
            workspace_id: Uuid,
            _now: time::OffsetDateTime,
        ) -> Result<Option<AccountActorState>, StoreError> {
            if self.fail {
                return Err(StoreError::Unavailable);
            }
            Ok(self
                .actor
                .clone()
                .filter(|actor| actor.credential_id == session_id)
                .filter(|actor| actor.workspace_id == workspace_id))
        }

        async fn resolve_account_token_central(
            &self,
            _token_id: Uuid,
            _now: time::OffsetDateTime,
        ) -> Result<Option<CentralActorState>, StoreError> {
            Ok(None)
        }

        async fn resolve_dashboard_session_central(
            &self,
            _session_id: Uuid,
            _now: time::OffsetDateTime,
        ) -> Result<Option<CentralActorState>, StoreError> {
            Ok(None)
        }

        async fn verification_key_set(&self) -> Result<Vec<SigningKeyRecord>, StoreError> {
            Ok(Vec::new())
        }

        async fn active_signing_key(&self) -> Result<SigningKeyRecord, StoreError> {
            Err(StoreError::NotFound)
        }
    }

    fn authority(reader: Reader) -> AssertionAuthority<Reader, LocalSigner> {
        AssertionAuthority::new(reader, signer(), ring(), &config())
    }

    fn key_request(digest: CredentialDigest, region: Region) -> Invocation {
        Invocation::WorkspaceKey(Box::new(ResolveWorkspaceKey {
            schema_version: SchemaVersion::V1,
            key: aex_wire::ids::ApiKeyId::from_uuid7(
                aex_wire::Uuid7::from_bytes(*uuid7(key_uuid()).as_bytes()).expect("a v7 payload"),
            ),
            presented_digest: digest,
            region,
            audience: AssertionAudience::RegionalSession,
        }))
    }

    /// The stub rows are keyed by raw UUID, and a prefixed id only wraps a v7
    /// payload, so the two have to agree on the same bytes.
    fn uuid7(value: Uuid) -> Uuid {
        let mut bytes = *value.as_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    }

    struct Fresh;

    impl EpochProjection for Fresh {
        fn projected(&self, _kind: aex_control_domain::EpochSubjectKind, _id: Uuid) -> Epoch {
            Epoch::NEVER
        }
    }

    fn issued_envelope(response: &AssertionResponse) -> Assertion {
        match response {
            AssertionResponse::Issued { assertion } => {
                Assertion::from_issued(assertion).expect("a well-formed envelope")
            }
            AssertionResponse::Refused { reason } => {
                panic!("expected an issue, got a refusal: {reason:?}")
            }
        }
    }

    fn inputs(binding: &[u8; 32]) -> VerificationInputs<'_> {
        VerificationInputs {
            now_ms: u64::try_from(NOW_MS).expect("positive"),
            audience: Audience {
                plane: Plane::Dev,
                region: Region::EuWest1,
                service: AssertionAudience::RegionalSession,
            },
            credential_binding: binding,
            projection: &Fresh,
        }
    }

    fn keys() -> VerificationKeySet {
        VerificationKeySet::new(vec![VerificationKey {
            kid: KeyId::new(Uuid::from_u128(9)),
            public_key: signer().public_key(),
            not_after_ms: u64::MAX,
        }])
        .expect("one key")
    }

    #[tokio::test]
    async fn a_matching_digest_issues_an_envelope_the_named_edge_verifies() {
        let mut key = key_state();
        key.key_id = uuid7(key_uuid());
        let response = authority(Reader {
            key: Some(key),
            ..Reader::default()
        })
        .answer(
            &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
            now(),
        )
        .await
        .expect("no fault");
        let assertion = issued_envelope(&response);

        let binding = workspace_key_binding(uuid7(key_uuid()), &digest());
        let claims = verify(
            assertion.as_bytes(),
            &keys(),
            &VerificationInputs {
                now_ms: u64::try_from(NOW_MS).expect("positive"),
                audience: Audience {
                    plane: Plane::Dev,
                    region: Region::EuWest1,
                    service: AssertionAudience::RegionalSession,
                },
                credential_binding: &binding,
                projection: &Fresh,
            },
        )
        .expect("the named edge verifies it");
        assert_eq!(claims.principal_kind, PrincipalKind::WorkspaceKey);
        assert_eq!(claims.workspace_id, workspace_uuid());
        // Effective scopes are computed here and never re-derived at the edge.
        assert_eq!(claims.scopes, ScopeSet::WORKSPACE_KEY_MINTABLE);
    }

    #[tokio::test]
    async fn an_assertion_minted_for_one_edge_is_refused_by_another() {
        let mut key = key_state();
        key.key_id = uuid7(key_uuid());
        let response = authority(Reader {
            key: Some(key),
            ..Reader::default()
        })
        .answer(
            &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
            now(),
        )
        .await
        .expect("no fault");
        let assertion = issued_envelope(&response);
        let binding = workspace_key_binding(uuid7(key_uuid()), &digest());
        for other in [
            AssertionAudience::RegionalSecret,
            AssertionAudience::RegionalObservation,
            AssertionAudience::RegionalOtlp,
            AssertionAudience::RegionalStream,
        ] {
            assert_eq!(
                verify(
                    assertion.as_bytes(),
                    &keys(),
                    &VerificationInputs {
                        now_ms: u64::try_from(NOW_MS).expect("positive"),
                        audience: Audience {
                            plane: Plane::Dev,
                            region: Region::EuWest1,
                            service: other,
                        },
                        credential_binding: &binding,
                        projection: &Fresh,
                    },
                ),
                Err(VerifyError::AudienceMismatch),
                "{other:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_wrong_digest_refuses_and_issues_nothing() {
        let mut key = key_state();
        key.key_id = uuid7(key_uuid());
        let response = authority(Reader {
            key: Some(key),
            ..Reader::default()
        })
        .answer(
            &key_request(CredentialDigest::new([0_u8; 32]), Region::EuWest1),
            now(),
        )
        .await
        .expect("no fault");
        assert_eq!(
            response,
            AssertionResponse::Refused {
                reason: AssertionRefusal::NotAuthorized
            }
        );
    }

    #[tokio::test]
    async fn an_unknown_key_is_indistinguishable_from_a_wrong_digest() {
        let unknown = authority(Reader::default())
            .answer(
                &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
                now(),
            )
            .await
            .expect("no fault");
        assert_eq!(
            unknown,
            AssertionResponse::Refused {
                reason: AssertionRefusal::NotAuthorized
            }
        );
    }

    #[tokio::test]
    async fn a_key_minted_for_another_region_never_issues() {
        let mut key = key_state();
        key.key_id = uuid7(key_uuid());
        key.region = Region::UsEast1;
        let response = authority(Reader {
            key: Some(key),
            ..Reader::default()
        })
        .answer(
            &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
            now(),
        )
        .await
        .expect("no fault");
        assert_eq!(
            response,
            AssertionResponse::Refused {
                reason: AssertionRefusal::NotAuthorized
            }
        );
    }

    #[tokio::test]
    async fn a_revoked_key_and_an_unreadable_account_answer_differently() {
        let mut revoked = key_state();
        revoked.key_id = uuid7(key_uuid());
        revoked.key_revoked = true;
        assert_eq!(
            authority(Reader {
                key: Some(revoked),
                ..Reader::default()
            })
            .answer(
                &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
                now()
            )
            .await
            .expect("no fault"),
            AssertionResponse::Refused {
                reason: AssertionRefusal::NotAuthorized
            }
        );

        // An unreadable account state is never downgraded to `active`: the
        // caller answers `503`, not `401`, and certainly not `200`.
        let mut unavailable = key_state();
        unavailable.key_id = uuid7(key_uuid());
        unavailable.account_state = AccountState::Unavailable;
        assert_eq!(
            authority(Reader {
                key: Some(unavailable),
                ..Reader::default()
            })
            .answer(
                &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
                now()
            )
            .await
            .expect("no fault"),
            AssertionResponse::Refused {
                reason: AssertionRefusal::AccountStateUnavailable
            }
        );
    }

    #[tokio::test]
    async fn an_unverifiable_credential_is_a_fault_and_never_a_refusal() {
        // A stored verifier under a pepper this process does not hold means the
        // credential cannot be checked. Answering `not_authorized` would tell a
        // customer their key is wrong when the truth is that we lost the pepper.
        let mut key = key_state();
        key.key_id = uuid7(key_uuid());
        key.pepper_version = 7;
        assert_eq!(
            authority(Reader {
                key: Some(key),
                ..Reader::default()
            })
            .answer(
                &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
                now()
            )
            .await,
            Err(IssueFault::UnknownPepper { version: 7 })
        );

        // An unreachable authority is a fault for the same reason.
        assert!(matches!(
            authority(Reader {
                fail: true,
                ..Reader::default()
            })
            .answer(
                &key_request(CredentialDigest::new(*digest().as_bytes()), Region::EuWest1),
                now()
            )
            .await,
            Err(IssueFault::Store(_))
        ));
    }

    fn session_request(user: Uuid) -> Invocation {
        Invocation::BrowserSession(Box::new(ResolveSessionForWorkspace {
            schema_version: SchemaVersion::V1,
            browser_session: aex_wire::ids::SessionId::from_uuid7(
                aex_wire::Uuid7::from_bytes(*uuid7(session_uuid()).as_bytes())
                    .expect("a v7 payload"),
            ),
            user: aex_wire::ids::UserId::from_uuid7(
                aex_wire::Uuid7::from_bytes(*uuid7(user).as_bytes()).expect("a v7 payload"),
            ),
            workspace: aex_wire::ids::WorkspaceId::from_uuid7(
                aex_wire::Uuid7::from_bytes(*uuid7(workspace_uuid()).as_bytes())
                    .expect("a v7 payload"),
            ),
            presented_digest: CredentialDigest::new(*digest().as_bytes()),
            region: Region::EuWest1,
            audience: AssertionAudience::RegionalSession,
        }))
    }

    #[tokio::test]
    async fn a_browser_session_issues_the_same_envelope_bound_to_its_own_credential() {
        let mut actor = actor_state();
        actor.credential_id = uuid7(session_uuid());
        actor.user_id = uuid7(user_uuid());
        actor.workspace_id = uuid7(workspace_uuid());
        let response = authority(Reader {
            actor: Some(actor),
            ..Reader::default()
        })
        .answer(&session_request(user_uuid()), now())
        .await
        .expect("no fault");
        let assertion = issued_envelope(&response);

        // The binding is domain-separated by principal kind, so an envelope
        // minted for a session can never be presented as a workspace key with
        // the same token digest.
        let session_binding = user_session_binding(uuid7(session_uuid()), &digest());
        let key_binding = workspace_key_binding(uuid7(session_uuid()), &digest());
        assert_ne!(session_binding, key_binding);
        let claims = verify(assertion.as_bytes(), &keys(), &inputs(&session_binding))
            .expect("the session binding verifies");
        assert_eq!(claims.principal_kind, PrincipalKind::UserSession);
        assert_eq!(
            verify(assertion.as_bytes(), &keys(), &inputs(&key_binding)),
            Err(VerifyError::CredentialBindingMismatch)
        );
    }

    #[tokio::test]
    async fn a_session_that_resolves_to_another_person_never_issues() {
        let mut actor = actor_state();
        actor.credential_id = uuid7(session_uuid());
        actor.user_id = uuid7(user_uuid());
        actor.workspace_id = uuid7(workspace_uuid());
        assert_eq!(
            authority(Reader {
                actor: Some(actor),
                ..Reader::default()
            })
            .answer(&session_request(Uuid::from_u128(0x99)), now())
            .await
            .expect("no fault"),
            AssertionResponse::Refused {
                reason: AssertionRefusal::NotAuthorized
            }
        );
    }

    #[test]
    fn an_unrecognised_payload_is_named_rather_than_refused() {
        // The API Gateway authorizer event is exactly such a payload. Answering
        // `not_authorized` would hide a missing contract behind a plausible
        // authorization failure.
        for payload in [
            serde_json::json!({}),
            serde_json::json!({"type": "REQUEST", "methodArn": "arn:aws:execute-api:..."}),
            serde_json::json!("a string"),
            serde_json::json!([1, 2, 3]),
        ] {
            assert_eq!(
                Invocation::classify(&payload),
                Err(InvocationError::Unrecognised),
                "{payload}"
            );
        }
    }

    #[test]
    fn a_named_but_malformed_operation_is_not_silently_reinterpreted() {
        let error = Invocation::classify(&serde_json::json!({"key": "not-an-id"}))
            .expect_err("a malformed request");
        assert!(
            matches!(error, InvocationError::Malformed { operation, .. } if operation == "ResolveWorkspaceKey"),
            "{error:?}"
        );
        let error = Invocation::classify(&serde_json::json!({"browserSession": 7}))
            .expect_err("a malformed request");
        assert!(
            matches!(error, InvocationError::Malformed { operation, .. } if operation == "ResolveSessionForWorkspace"),
            "{error:?}"
        );
    }

    #[test]
    fn the_pepper_ring_refuses_every_unusable_document() {
        let ring = ring();
        assert_eq!(ring.len(), 1);
        assert!(!ring.is_empty());

        for (document, name) in [
            (r#"{"schemaVersion":1,"peppers":[]}"#.to_owned(), "empty"),
            (
                pepper_document(&[(1, PEPPER), (1, [2_u8; 32])]),
                "duplicate version",
            ),
            (
                r#"{"peppers":[{"version":1,"material":"AAAA"}]}"#.to_owned(),
                "unversioned",
            ),
            (
                r#"{"schemaVersion":1,"peppers":[{"version":1,"material":"AAAA"}],"extra":1}"#
                    .to_owned(),
                "unknown member",
            ),
            (
                r#"{"schemaVersion":1,"peppers":[{"version":1,"material":"short"}]}"#.to_owned(),
                "short material",
            ),
        ] {
            assert!(
                parse_pepper_ring("test", &document).is_err(),
                "a {name} document must stop the process"
            );
        }

        let too_many: Vec<(u16, [u8; 32])> = (0..=u16::try_from(MAX_PEPPERS).expect("small"))
            .map(|version| (version, [u8::try_from(version % 251).expect("small"); 32]))
            .collect();
        assert!(matches!(
            parse_pepper_ring("test", &pepper_document(&too_many)),
            Err(SecretError::TooManyPeppers { .. })
        ));
    }

    #[test]
    fn a_signing_secret_is_exactly_thirty_two_bytes() {
        let material = aex_control_domain::codec::base64url(&[5_u8; 32]);
        assert_eq!(
            parse_signing_secret("test", &format!("  {material}\n")).expect("a usable secret"),
            [5_u8; 32]
        );
        for bad in [
            "",
            "not base64url!",
            &aex_control_domain::codec::base64url(&[5_u8; 31]),
        ] {
            assert!(
                matches!(
                    parse_signing_secret("test", bad),
                    Err(SecretError::Material { .. })
                ),
                "{bad}"
            );
        }
    }
}
