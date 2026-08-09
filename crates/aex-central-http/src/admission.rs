//! In-process credential admission for the central plane.
//!
//! # Why this module exists
//!
//! Behind API Gateway a central process never sees a credential. The gateway
//! invokes `central-authz`, which resolves the credential against Aurora and
//! returns the flat context map [`crate::authorizer`] turns into a principal.
//! That is sound *only* because the map can have come from nowhere else.
//!
//! A load balancer supplies no such guarantee: every header is caller-authored.
//! A central process reachable through an ALB that still trusts an ambient
//! context map is not authenticating at all — it is accepting whatever principal
//! the caller claims. This module is what replaces the gateway's authorizer, so
//! that the process itself resolves and **verifies** the credential.
//!
//! # This is a relocation, not a new mechanism
//!
//! Every check below is `services/central-authz/src/authorizer.rs`'s, in its
//! order, over the same ports:
//!
//! * [`bearer`] is that module's `bearer`: one unambiguous `Bearer` value;
//! * [`parse_supported`] is its `parse_supported`: three central credential
//!   kinds by prefix and nothing else;
//! * [`CredentialAdmission::authenticate`] is its `answer`: resolve the row,
//!   echo-check the id, check the workspace and region pin, revocation, expiry,
//!   user activity, workspace and organization status, and last the MAC;
//! * [`CredentialAdmission::matches`] is its `credential_matches`: the peppered
//!   MAC verify `verify(pepper, &digest, &Verifier::from_bytes(stored))`,
//!   resolving the pepper by the version **the stored row names**.
//!
//! No cryptography is authored here. The one deliberate change is where the
//! pepper comes from: [`aex_identity_app::ports::PepperKeystore`] rather than a
//! flat ring, because the port resolves the exact version a stored verifier was
//! computed under and therefore survives a rotation that a
//! "whatever the secret holds now" read would break.
//!
//! # Refusal versus fault
//!
//! Two outcomes, and conflating them is the failure this module is most careful
//! about:
//!
//! * **Refusal** — unknown, mismatched, revoked, lapsed, inactive, wrongly
//!   pinned, or a MAC that does not verify. All of them are one
//!   [`EdgeError::CredentialRefused`], so a caller cannot learn which check
//!   failed.
//! * **Fault** — the store did not answer, or the pepper the row names cannot be
//!   resolved here. Both are [`EdgeError::AuthenticationUnavailable`], a
//!   retryable `503`. Answering "your credential is invalid" when the truth is
//!   "we could not check" is a lie the caller acts on by discarding a credential
//!   that works.

use std::sync::Arc;

use aex_control_app::ports::{AuthorizationReader, CentralActorState, WorkspaceKeyState};
use aex_control_domain::{AccountState, OrganizationStatus, WorkspaceStatus};
use aex_identity_app::ports::{PepperKeystore, PepperPurpose};
use aex_identity_domain::credential::{
    CredentialKind, ParsedCredential, Pepper, PepperVersion, Verifier, parse, verify,
};
use aex_wire::types::RequestId;
use async_trait::async_trait;
use http::HeaderMap;
use time::OffsetDateTime;

use crate::authorizer::{CentralAuthorizerContext, ContextPrincipalKind, MAX_CONTEXT_LIFETIME_MS};
use crate::error::EdgeError;

/// The header a caller may use to name its own request identity.
///
/// The same spelling `finance-api`'s edge already accepts. It is a diagnostic
/// correlation id and never an authorization input: nothing below reads it, and
/// the context it lands in carries no authority of its own.
pub const REQUEST_ID_HEADER: &str = "aex-request-id";

/// What the router asks before it admits a request.
///
/// `Ok(None)` means **no credential was presented at all**, which the two
/// device-flow routes admit and every other route refuses. A credential that was
/// presented and not admitted is `Err`, never `Ok(None)`: a process that
/// silently downgrades a rejected credential to "anonymous" is a process whose
/// authentication can be skipped by sending garbage.
#[async_trait]
pub trait CentralAuthenticator: Send + Sync {
    /// Resolves the presented credential into a verified context.
    ///
    /// # Errors
    ///
    /// Returns [`EdgeError::CredentialRefused`] for any refusal, and
    /// [`EdgeError::AuthenticationUnavailable`] when the answer is unknown
    /// rather than negative.
    async fn authenticate(
        &self,
        headers: &HeaderMap,
        now: OffsetDateTime,
    ) -> Result<Option<CentralAuthorizerContext>, EdgeError>;
}

/// Where the pepper for one credential kind comes from.
///
/// `None` is always a **fault**, never a refusal: it says the credential could
/// not be checked, not that it is wrong.
#[async_trait]
pub trait CredentialPeppers: Send + Sync {
    /// The pepper a verifier of this kind at this version was computed under.
    async fn pepper(&self, kind: CredentialKind, version: PepperVersion) -> Option<Pepper>;
}

/// The two purposed keystores, addressed by credential kind.
///
/// The mapping lives here, once, rather than at each call site: a workspace key
/// is peppered under [`PepperPurpose::ApiKey`] by the control authority, and
/// every actor credential under [`PepperPurpose::Identity`] by the identity
/// authority. Getting that pairing wrong makes every credential of one kind
/// unverifiable, which this type exists to make a single reviewable decision.
pub struct PurposedPeppers {
    identity: Arc<dyn PepperKeystore>,
    api_key: Arc<dyn PepperKeystore>,
}

impl std::fmt::Debug for PurposedPeppers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PurposedPeppers").finish()
    }
}

impl PurposedPeppers {
    /// Binds the identity and API-key keystores.
    #[must_use]
    pub const fn new(identity: Arc<dyn PepperKeystore>, api_key: Arc<dyn PepperKeystore>) -> Self {
        Self { identity, api_key }
    }

    /// Which purpose peppers a credential of this kind.
    #[must_use]
    pub const fn purpose(kind: CredentialKind) -> PepperPurpose {
        match kind {
            CredentialKind::WorkspaceKey => PepperPurpose::ApiKey,
            CredentialKind::AccountToken
            | CredentialKind::DashboardSession
            | CredentialKind::EmailChallenge
            | CredentialKind::DeviceCode => PepperPurpose::Identity,
        }
    }
}

#[async_trait]
impl CredentialPeppers for PurposedPeppers {
    async fn pepper(&self, kind: CredentialKind, version: PepperVersion) -> Option<Pepper> {
        let purpose = Self::purpose(kind);
        let keystore = match purpose {
            PepperPurpose::ApiKey => &self.api_key,
            PepperPurpose::Identity | PepperPurpose::Cursor => &self.identity,
        };
        keystore.by_version(purpose, version).await.ok()
    }
}

/// The in-process replacement for the API Gateway REQUEST authorizer.
pub struct CredentialAdmission<R, P> {
    reader: R,
    peppers: P,
    context_lifetime_ms: u64,
}

impl<R, P> std::fmt::Debug for CredentialAdmission<R, P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialAdmission")
            .field("context_lifetime_ms", &self.context_lifetime_ms)
            .finish_non_exhaustive()
    }
}

impl<R, P> CredentialAdmission<R, P>
where
    R: AuthorizationReader,
    P: CredentialPeppers,
{
    /// Binds the authorization reader and the pepper source.
    ///
    /// `context_lifetime_ms` is clamped to [`MAX_CONTEXT_LIFETIME_MS`] the same
    /// way `central-authz` clamps it, so a configuration mistake cannot widen
    /// the window a decision is honoured for.
    #[must_use]
    pub const fn new(reader: R, peppers: P, context_lifetime_ms: u64) -> Self {
        Self {
            reader,
            peppers,
            context_lifetime_ms,
        }
    }

    /// The peppered MAC verify.
    ///
    /// `Ok(false)` is a refusal; `Err` is a fault, because a pepper this process
    /// cannot resolve makes the credential *unverifiable* rather than invalid.
    async fn matches(
        &self,
        credential: &ParsedCredential,
        version: u16,
        stored: [u8; 32],
    ) -> Result<bool, EdgeError> {
        let pepper = self
            .peppers
            .pepper(credential.kind, PepperVersion::new(version))
            .await
            .ok_or(EdgeError::AuthenticationUnavailable)?;
        Ok(verify(
            &pepper,
            &credential.digest,
            &Verifier::from_bytes(stored),
        ))
    }

    async fn admit_workspace_key(
        &self,
        credential: &ParsedCredential,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<CentralAuthorizerContext, EdgeError> {
        let state = self
            .reader
            .resolve_workspace_key(credential.id)
            .await
            .map_err(|_| EdgeError::AuthenticationUnavailable)?
            .ok_or(EdgeError::CredentialRefused)?;
        // The token names its own region and workspace. Both are compared
        // against the key's row rather than trusted: a token whose pin disagrees
        // with the key it names is refused outright, so the segments a later
        // stage reads to address rows can never describe a key that lives
        // somewhere else.
        let pin = credential.pin;
        if state.key_id != credential.id
            || pin.map(|pin| pin.region.region()) != Some(state.region)
            || pin.map(|pin| pin.workspace) != Some(state.workspace_id)
            || state.key_revoked
            || state.workspace_status != WorkspaceStatus::Active
            || state.organization_status != OrganizationStatus::Active
            || !self
                .matches(credential, state.pepper_version, state.verifier)
                .await?
        {
            return Err(EdgeError::CredentialRefused);
        }
        self.context_for_key(request_id, &state, now)
    }

    async fn admit_actor(
        &self,
        credential: &ParsedCredential,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<CentralAuthorizerContext, EdgeError> {
        let state = match credential.kind {
            CredentialKind::AccountToken => {
                self.reader
                    .resolve_account_token_central(credential.id, now)
                    .await
            }
            CredentialKind::DashboardSession => {
                self.reader
                    .resolve_dashboard_session_central(credential.id, now)
                    .await
            }
            CredentialKind::WorkspaceKey
            | CredentialKind::EmailChallenge
            | CredentialKind::DeviceCode => return Err(EdgeError::CredentialRefused),
        }
        .map_err(|_| EdgeError::AuthenticationUnavailable)?
        .ok_or(EdgeError::CredentialRefused)?;
        if state.credential_id != credential.id
            || state.credential_revoked
            || state.credential_expired
            || !state.user_active
            || !self
                .matches(credential, state.pepper_version, state.verifier)
                .await?
        {
            return Err(EdgeError::CredentialRefused);
        }
        self.context_for_actor(credential.kind, request_id, state, now)
    }

    /// The window a decision is honoured for, clamped to the ceiling.
    fn window(&self, now: OffsetDateTime) -> (i64, i64) {
        let issued_at_ms =
            i64::try_from(now.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX);
        let bounded = i64::try_from(self.context_lifetime_ms)
            .unwrap_or(MAX_CONTEXT_LIFETIME_MS)
            .clamp(1, MAX_CONTEXT_LIFETIME_MS);
        (issued_at_ms, issued_at_ms.saturating_add(bounded))
    }

    fn context_for_key(
        &self,
        request_id: RequestId,
        state: &WorkspaceKeyState,
        now: OffsetDateTime,
    ) -> Result<CentralAuthorizerContext, EdgeError> {
        let (issued_at_ms, expires_at_ms) = self.window(now);
        CentralAuthorizerContext {
            request_id,
            kind: ContextPrincipalKind::WorkspaceKey,
            principal_id: state.key_id,
            credential_id: Some(state.key_id),
            workspace_id: Some(state.workspace_id),
            organization_id: Some(state.organization_id),
            region: Some(state.region),
            memberships: Vec::new(),
            scopes: state.scopes,
            account_state: state.account_state,
            issued_at_ms,
            expires_at_ms,
        }
        .checked()
        .map_err(EdgeError::Context)
    }

    fn context_for_actor(
        &self,
        kind: CredentialKind,
        request_id: RequestId,
        state: CentralActorState,
        now: OffsetDateTime,
    ) -> Result<CentralAuthorizerContext, EdgeError> {
        let (issued_at_ms, expires_at_ms) = self.window(now);
        let principal_kind = match kind {
            CredentialKind::AccountToken => ContextPrincipalKind::Account,
            CredentialKind::DashboardSession => ContextPrincipalKind::UserSession,
            CredentialKind::WorkspaceKey
            | CredentialKind::EmailChallenge
            | CredentialKind::DeviceCode => {
                return Err(EdgeError::Internal("an actor context needs an actor kind"));
            }
        };
        CentralAuthorizerContext {
            request_id,
            kind: principal_kind,
            principal_id: state.user_id,
            credential_id: Some(state.credential_id),
            workspace_id: None,
            organization_id: None,
            region: None,
            memberships: state.memberships,
            scopes: state.scopes,
            // One actor can belong to several organizations. Claiming one
            // account state here would be false; central admission reads the
            // selected organization's current state after authorization.
            account_state: AccountState::Unavailable,
            issued_at_ms,
            expires_at_ms,
        }
        .checked()
        .map_err(EdgeError::Context)
    }
}

#[async_trait]
impl<R, P> CentralAuthenticator for CredentialAdmission<R, P>
where
    R: AuthorizationReader,
    P: CredentialPeppers,
{
    async fn authenticate(
        &self,
        headers: &HeaderMap,
        now: OffsetDateTime,
    ) -> Result<Option<CentralAuthorizerContext>, EdgeError> {
        let Some(raw) = bearer(headers) else {
            // `bearer` answers `None` both for "no header" and for "a header
            // that is not one unambiguous Bearer value", and those are not the
            // same request. Nothing presented lets the route decide — the two
            // device-flow routes admit it. A header that *was* sent and did not
            // resolve is a refusal, because treating it as absent would make a
            // route that admits anonymous callers also admit a malformed
            // credential as one.
            return if headers.contains_key(http::header::AUTHORIZATION) {
                Err(EdgeError::CredentialRefused)
            } else {
                Ok(None)
            };
        };
        // A header that *is* present and is not a credential this plane serves
        // is refused here rather than downgraded to "no credential". Silently
        // ignoring a rejected credential is how authentication becomes optional.
        let credential = parse_supported(raw).ok_or(EdgeError::CredentialRefused)?;
        let request_id = request_id(headers);
        let context = match credential.kind {
            CredentialKind::WorkspaceKey => {
                self.admit_workspace_key(&credential, request_id, now)
                    .await?
            }
            CredentialKind::AccountToken | CredentialKind::DashboardSession => {
                self.admit_actor(&credential, request_id, now).await?
            }
            CredentialKind::EmailChallenge | CredentialKind::DeviceCode => {
                return Err(EdgeError::CredentialRefused);
            }
        };
        Ok(Some(context))
    }
}

/// The one unambiguous `Authorization: Bearer <value>`.
///
/// Relocated from `central-authz`, unchanged. Two values, a comma, an empty
/// credential or embedded whitespace all answer `None`, so a caller cannot
/// present two credentials and have the edge pick one.
#[must_use]
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(http::header::AUTHORIZATION).iter();
    let one = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let value = one.to_str().ok()?;
    if value.contains(',') {
        return None;
    }
    let (scheme, credential) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || credential.is_empty()
        || credential.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(credential)
}

/// The three credential kinds the central plane resolves.
///
/// Relocated from `central-authz`, unchanged. `EmailChallenge` and `DeviceCode`
/// are redeemed by a ceremony, never presented as a bearer credential, so they
/// are not admitted here even though they parse elsewhere.
#[must_use]
pub fn parse_supported(raw: &str) -> Option<ParsedCredential> {
    let kind = if raw.starts_with(CredentialKind::WorkspaceKey.prefix()) {
        CredentialKind::WorkspaceKey
    } else if raw.starts_with(CredentialKind::AccountToken.prefix()) {
        CredentialKind::AccountToken
    } else if raw.starts_with(CredentialKind::DashboardSession.prefix()) {
        CredentialKind::DashboardSession
    } else {
        return None;
    };
    parse(kind, raw).ok()
}

/// The request identity this request is traced under.
///
/// A caller may name its own so a client trace and a server trace agree. It is
/// diagnostic only: no authorization decision below reads it, and the gateway's
/// `requestContext.requestId` it replaces was never an authorization input
/// either.
#[must_use]
pub fn request_id(headers: &HeaderMap) -> RequestId {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| RequestId::parse(value).ok())
        .unwrap_or_else(minted_request_id)
}

/// A fresh request identity.
///
/// # Panics
///
/// Never: the minted spelling satisfies the `RequestId` grammar.
fn minted_request_id() -> RequestId {
    RequestId::parse(&format!("req_{}", uuid::Uuid::now_v7().simple()))
        .unwrap_or_else(|error| unreachable!("a minted request id is valid: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        CentralAuthenticator, CredentialAdmission, CredentialPeppers, PurposedPeppers,
        REQUEST_ID_HEADER, bearer, parse_supported, request_id,
    };
    use crate::authorizer::ContextPrincipalKind;
    use crate::error::EdgeError;
    use aex_control_app::ports::{
        AccountActorState, AuthorizationReader, CentralActorState, SigningKeyRecord, StoreError,
        WorkspaceKeyState,
    };
    use aex_control_domain::{AccountState, Epoch, OrganizationStatus, ScopeSet, WorkspaceStatus};
    use aex_identity_app::ports::PepperPurpose;
    use aex_identity_domain::credential::{
        CredentialKind, Pepper, PepperVersion, PresentedDigest, RegionCode, SecretRng,
        WorkspacePin, mint, verifier,
    };
    use aex_wire::types::Region;
    use async_trait::async_trait;
    use http::HeaderMap;
    use time::OffsetDateTime;
    use uuid::Uuid;

    const PEPPER_BYTES: [u8; 32] = [17; 32];
    const KEY: u128 = 0x31;
    const WORKSPACE: u128 = 0x32;
    const ORGANIZATION: u128 = 0x33;
    const TOKEN: u128 = 0x11;
    const USER: u128 = 0x22;

    #[derive(Debug)]
    struct FixedRng;

    impl SecretRng for FixedRng {
        fn fill(&self, out: &mut [u8]) {
            out.fill(23);
        }
    }

    /// A ring holding exactly version 1, for whichever purpose is asked.
    #[derive(Debug, Clone, Copy)]
    struct OnePepper;

    #[async_trait]
    impl CredentialPeppers for OnePepper {
        async fn pepper(&self, _kind: CredentialKind, version: PepperVersion) -> Option<Pepper> {
            (version.get() == 1).then(|| Pepper::new(PEPPER_BYTES))
        }
    }

    /// Every pepper lookup is unresolvable, which must be a fault.
    #[derive(Debug, Clone, Copy)]
    struct NoPepper;

    #[async_trait]
    impl CredentialPeppers for NoPepper {
        async fn pepper(&self, _kind: CredentialKind, _version: PepperVersion) -> Option<Pepper> {
            None
        }
    }

    #[derive(Debug, Default, Clone)]
    struct Reader {
        actor: Option<CentralActorState>,
        key: Option<WorkspaceKeyState>,
        unreachable: bool,
    }

    #[async_trait]
    impl AuthorizationReader for Reader {
        async fn resolve_workspace_key(
            &self,
            _key_id: Uuid,
        ) -> Result<Option<WorkspaceKeyState>, StoreError> {
            if self.unreachable {
                return Err(StoreError::Unavailable);
            }
            Ok(self.key.clone())
        }

        async fn resolve_account_token_for_workspace(
            &self,
            _token_id: Uuid,
            _workspace_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<AccountActorState>, StoreError> {
            Ok(None)
        }

        async fn resolve_session_for_workspace(
            &self,
            _session_id: Uuid,
            _workspace_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<AccountActorState>, StoreError> {
            Ok(None)
        }

        async fn resolve_account_token_central(
            &self,
            _token_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<CentralActorState>, StoreError> {
            if self.unreachable {
                return Err(StoreError::Unavailable);
            }
            Ok(self.actor.clone())
        }

        async fn resolve_dashboard_session_central(
            &self,
            _session_id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<Option<CentralActorState>, StoreError> {
            if self.unreachable {
                return Err(StoreError::Unavailable);
            }
            Ok(self.actor.clone())
        }

        async fn verification_key_set(&self) -> Result<Vec<SigningKeyRecord>, StoreError> {
            Ok(Vec::new())
        }

        async fn active_signing_key(&self) -> Result<SigningKeyRecord, StoreError> {
            Err(StoreError::NotFound)
        }
    }

    fn actor_state(digest: &PresentedDigest) -> CentralActorState {
        CentralActorState {
            credential_id: Uuid::from_u128(TOKEN),
            user_id: Uuid::from_u128(USER),
            scopes: ScopeSet::CENTRAL,
            verifier: *verifier(&Pepper::new(PEPPER_BYTES), digest).as_bytes(),
            pepper_version: 1,
            credential_revoked: false,
            credential_expired: false,
            user_active: true,
            memberships: Vec::new(),
        }
    }

    fn key_state(digest: &PresentedDigest) -> WorkspaceKeyState {
        WorkspaceKeyState {
            key_id: Uuid::from_u128(KEY),
            workspace_id: Uuid::from_u128(WORKSPACE),
            organization_id: Uuid::from_u128(ORGANIZATION),
            scopes: ScopeSet::ALL,
            verifier: *verifier(&Pepper::new(PEPPER_BYTES), digest).as_bytes(),
            pepper_version: 1,
            key_revoked: false,
            region: Region::EuWest1,
            workspace_status: WorkspaceStatus::Active,
            organization_status: OrganizationStatus::Active,
            account_state: AccountState::Active,
            epoch_key: Epoch::NEVER,
            epoch_workspace: Epoch::NEVER,
            epoch_account: Epoch::NEVER,
        }
    }

    fn headers(authorization: Option<&str>) -> HeaderMap {
        let mut map = HeaderMap::new();
        if let Some(value) = authorization {
            map.insert(
                http::header::AUTHORIZATION,
                format!("Bearer {value}").parse().expect("a header value"),
            );
        }
        map
    }

    fn admission(reader: Reader) -> CredentialAdmission<Reader, OnePepper> {
        CredentialAdmission::new(reader, OnePepper, 30_000)
    }

    fn account_token() -> (String, PresentedDigest) {
        let (secret, digest) = mint(
            CredentialKind::AccountToken,
            None,
            Uuid::from_u128(TOKEN),
            &FixedRng,
        );
        (secret.expose().to_owned(), digest)
    }

    fn workspace_key() -> (String, PresentedDigest) {
        let (secret, digest) = mint(
            CredentialKind::WorkspaceKey,
            Some(WorkspacePin {
                region: RegionCode::new(Region::EuWest1),
                workspace: Uuid::from_u128(WORKSPACE),
            }),
            Uuid::from_u128(KEY),
            &FixedRng,
        );
        (secret.expose().to_owned(), digest)
    }

    #[tokio::test]
    async fn a_valid_account_token_mints_the_context_the_edge_accepts() {
        let (secret, digest) = account_token();
        let context = admission(Reader {
            actor: Some(actor_state(&digest)),
            ..Reader::default()
        })
        .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("the authority answers")
        .expect("a credential was presented");
        assert_eq!(context.kind, ContextPrincipalKind::Account);
        assert_eq!(context.principal_id, Uuid::from_u128(USER));
        assert_eq!(context.credential_id, Some(Uuid::from_u128(TOKEN)));
        assert_eq!(context.account_state, AccountState::Unavailable);
        assert_eq!(context.expires_at_ms - context.issued_at_ms, 30_000);
    }

    #[tokio::test]
    async fn a_valid_workspace_key_carries_its_placement_and_no_membership() {
        let (secret, digest) = workspace_key();
        let context = admission(Reader {
            key: Some(key_state(&digest)),
            ..Reader::default()
        })
        .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("the authority answers")
        .expect("a credential was presented");
        assert_eq!(context.kind, ContextPrincipalKind::WorkspaceKey);
        assert_eq!(context.workspace_id, Some(Uuid::from_u128(WORKSPACE)));
        assert_eq!(context.organization_id, Some(Uuid::from_u128(ORGANIZATION)));
        assert_eq!(context.region, Some(Region::EuWest1));
        assert!(context.memberships.is_empty());
    }

    #[tokio::test]
    async fn a_forged_secret_against_a_real_row_is_refused_in_process() {
        // The row exists, the id echoes, nothing is revoked — only the MAC is
        // wrong. This is the case the gateway used to catch and an ALB does not.
        let (secret, digest) = account_token();
        let mut state = actor_state(&digest);
        state.verifier = [99; 32];
        assert_eq!(
            admission(Reader {
                actor: Some(state),
                ..Reader::default()
            })
            .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
            .await,
            Err(EdgeError::CredentialRefused)
        );
    }

    #[tokio::test]
    async fn a_forged_workspace_key_secret_is_refused_in_process() {
        let (secret, digest) = workspace_key();
        let mut state = key_state(&digest);
        state.verifier = [99; 32];
        assert_eq!(
            admission(Reader {
                key: Some(state),
                ..Reader::default()
            })
            .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
            .await,
            Err(EdgeError::CredentialRefused)
        );
    }

    #[tokio::test]
    async fn an_absent_credential_mints_no_context_at_all() {
        assert_eq!(
            admission(Reader::default())
                .authenticate(&headers(None), OffsetDateTime::UNIX_EPOCH)
                .await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn a_credential_shaped_like_nothing_this_plane_serves_is_refused_not_ignored() {
        for presented in ["not-a-token", "aex_dc_something", ""] {
            let mut map = HeaderMap::new();
            map.insert(
                http::header::AUTHORIZATION,
                format!("Bearer {presented}")
                    .parse()
                    .expect("a header value"),
            );
            assert_eq!(
                admission(Reader::default())
                    .authenticate(&map, OffsetDateTime::UNIX_EPOCH)
                    .await,
                Err(EdgeError::CredentialRefused),
                "{presented}"
            );
        }
    }

    #[tokio::test]
    async fn every_negative_row_state_gives_one_undifferentiated_refusal() {
        /// One negative mutation of an otherwise valid key row.
        type Mutation = Box<dyn Fn(&mut WorkspaceKeyState)>;

        let (secret, digest) = workspace_key();
        let mutations: Vec<(&str, Mutation)> = vec![
            ("revoked", Box::new(|state| state.key_revoked = true)),
            (
                "workspace deleting",
                Box::new(|state| state.workspace_status = WorkspaceStatus::Deleting),
            ),
            (
                "workspace provisioning",
                Box::new(|state| state.workspace_status = WorkspaceStatus::Provisioning),
            ),
            (
                "another workspace",
                Box::new(|state| state.workspace_id = Uuid::from_u128(0xDEAD)),
            ),
            (
                "another region",
                Box::new(|state| state.region = Region::UsEast1),
            ),
            (
                "another key id",
                Box::new(|state| state.key_id = Uuid::from_u128(0xBEEF)),
            ),
        ];
        for (label, mutate) in mutations {
            let mut state = key_state(&digest);
            mutate(&mut state);
            assert_eq!(
                admission(Reader {
                    key: Some(state),
                    ..Reader::default()
                })
                .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
                .await,
                Err(EdgeError::CredentialRefused),
                "{label}"
            );
        }
    }

    #[tokio::test]
    async fn an_unknown_credential_and_a_wrong_secret_are_the_same_public_answer() {
        let (secret, _) = account_token();
        let unknown = admission(Reader::default())
            .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
            .await;
        let (secret, digest) = account_token();
        let mut state = actor_state(&digest);
        state.verifier = [7; 32];
        let wrong = admission(Reader {
            actor: Some(state),
            ..Reader::default()
        })
        .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
        .await;
        assert_eq!(unknown, wrong);
        assert_eq!(unknown, Err(EdgeError::CredentialRefused));
    }

    #[tokio::test]
    async fn an_unreachable_store_is_retryable_and_never_a_refusal() {
        let (secret, _) = account_token();
        let failure = admission(Reader {
            unreachable: true,
            ..Reader::default()
        })
        .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
        .await
        .expect_err("an unreachable store is a failure");
        assert_eq!(failure, EdgeError::AuthenticationUnavailable);
        assert!(failure.code().retryable());
    }

    #[tokio::test]
    async fn a_pepper_this_process_cannot_resolve_is_a_fault_not_an_invalid_credential() {
        let (secret, digest) = account_token();
        let failure = CredentialAdmission::new(
            Reader {
                actor: Some(actor_state(&digest)),
                ..Reader::default()
            },
            NoPepper,
            30_000,
        )
        .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
        .await
        .expect_err("an unresolvable pepper is a failure");
        assert_eq!(failure, EdgeError::AuthenticationUnavailable);
    }

    #[tokio::test]
    async fn a_configured_lifetime_can_never_exceed_the_ceiling() {
        let (secret, digest) = account_token();
        let context = CredentialAdmission::new(
            Reader {
                actor: Some(actor_state(&digest)),
                ..Reader::default()
            },
            OnePepper,
            u64::MAX,
        )
        .authenticate(&headers(Some(&secret)), OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("the authority answers")
        .expect("a credential was presented");
        assert_eq!(
            context.expires_at_ms - context.issued_at_ms,
            crate::authorizer::MAX_CONTEXT_LIFETIME_MS
        );
    }

    #[test]
    fn two_authorization_values_never_resolve_to_one_credential() {
        let mut map = HeaderMap::new();
        map.append(
            http::header::AUTHORIZATION,
            "Bearer one".parse().expect("a header value"),
        );
        map.append(
            http::header::AUTHORIZATION,
            "Bearer two".parse().expect("a header value"),
        );
        assert_eq!(bearer(&map), None);
        assert_eq!(bearer(&headers(Some("aex_at_x"))), Some("aex_at_x"));
        let mut comma = HeaderMap::new();
        comma.insert(
            http::header::AUTHORIZATION,
            "Bearer one,two".parse().expect("a header value"),
        );
        assert_eq!(bearer(&comma), None);
    }

    #[test]
    fn only_the_three_central_credential_kinds_parse() {
        for kind in [
            CredentialKind::EmailChallenge,
            CredentialKind::DeviceCode,
            CredentialKind::WorkspaceKey,
            CredentialKind::AccountToken,
            CredentialKind::DashboardSession,
        ] {
            let (secret, _) = mint(kind, None, Uuid::from_u128(1), &FixedRng);
            let parsed = parse_supported(secret.expose());
            let central = matches!(
                kind,
                CredentialKind::WorkspaceKey
                    | CredentialKind::AccountToken
                    | CredentialKind::DashboardSession
            );
            // A workspace key minted without a pin does not parse either way;
            // what matters is that no non-central kind ever does.
            if !central {
                assert!(parsed.is_none(), "{kind:?} must not be admitted");
            }
        }
    }

    #[test]
    fn the_pepper_purpose_is_pinned_per_credential_kind() {
        assert_eq!(
            PurposedPeppers::purpose(CredentialKind::WorkspaceKey),
            PepperPurpose::ApiKey
        );
        for kind in [
            CredentialKind::AccountToken,
            CredentialKind::DashboardSession,
            CredentialKind::EmailChallenge,
            CredentialKind::DeviceCode,
        ] {
            assert_eq!(PurposedPeppers::purpose(kind), PepperPurpose::Identity);
        }
    }

    #[test]
    fn a_caller_may_name_its_own_request_identity_and_a_bad_one_is_replaced() {
        let mut map = HeaderMap::new();
        map.insert(
            REQUEST_ID_HEADER,
            "req-from-the-client".parse().expect("a header value"),
        );
        assert_eq!(request_id(&map).as_str(), "req-from-the-client");
        assert!(request_id(&HeaderMap::new()).as_str().starts_with("req_"));
    }
}
