//! The shared regional edge: the precedence stages every finite regional
//! deployable runs before the generated dispatcher sees a request.
//!
//! One implementation, three deployables. The stage order is
//! [`crate::router::EDGE_PRECEDENCE`] and each stage's decision comes from the
//! generated route descriptor, never from the handler: `required_scope`,
//! `pause_exempt` and `idempotency` are table facts.

use aex_control_domain::epoch::EpochSubjectKind;
use aex_identity_domain::assertion::{Audience, Plane, PrincipalKind, VerificationKeySet};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::idempotency::{IdempotencyKey, IdempotencyKind, PrincipalScope};
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::routes::{BodyClass, route};
use aex_wire::types::{ETag, Region, Timestamp};
use http::HeaderMap;
use sha2::Digest as _;
use uuid::Uuid;

use crate::assertion::{
    AssertionSource, AuthFailure, CredentialFloors, PresentedCredential, ProjectedEpochs,
    RegionalFloors, VerifiedAuthorization, VerifyingAssertionCache,
};
use crate::capacity::{LimitProjectionError, LimitResolver};
use crate::context::{AccountState, AuthorizationEpochs, RegionalAuthorization, RequestContext};
use crate::idempotency::{IdentityContext, identity, operation_id};
use crate::mount::{AdmissionRequest, EdgeAdmission};

/// The regional projection the edge consults before it trusts an assertion.
///
/// A revoked key, a paused account or an advanced revocation epoch is visible
/// regionally before the 30-second assertion expires, which is the only reason
/// a 30-second lifetime is safe.
///
/// # Why it takes two reads rather than one
///
/// The projection is keyed the way its writer keys it: revocation by API key,
/// placement by workspace. A credential names its own key and nothing else — the
/// workspace it belongs to is a fact only the assertion carries — so the two
/// facts become available at two different points and are read at those two
/// points. The alternative is a credential-to-workspace index nothing writes.
///
/// Both reads happen on **every** request. Neither is cached, which is what
/// keeps the 30-second assertion cache safe: a cached assertion still loses to a
/// revocation or a pause published a moment ago.
#[async_trait::async_trait]
pub trait ProjectionReader: Send + Sync + 'static {
    /// Reads the current revocation floor for a known workspace-key identity.
    ///
    /// Long-lived transports no longer retain credential plaintext after
    /// admission. The projected key identity is sufficient for every later
    /// revocation check and avoids retaining a bearer token for the socket life.
    async fn project_key(&self, key: ApiKeyId) -> Result<ProjectedEpochs, ProjectionError>;

    /// Reads the revocation floors bound to the presented credential alone.
    ///
    /// This runs before any assertion exists, so it can only speak for the
    /// credential: an absent revocation row is the zero floor, not a missing
    /// answer.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] rather than a stale answer: an unavailable
    /// projection fails the request closed.
    async fn project(
        &self,
        credential: &PresentedCredential,
    ) -> Result<ProjectedEpochs, ProjectionError>;

    /// Reads the placement of the workspace a verified assertion names.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Unknown`] when this region holds no placement
    /// for the workspace, and [`ProjectionError::Unavailable`] when it could not
    /// answer. Neither is ever downgraded to an optimistic `active`.
    async fn placement(&self, workspace: WorkspaceId) -> Result<ProjectedState, ProjectionError>;
}

/// What the regional projection says about one workspace right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectedState {
    /// The monotonic epochs an assertion must not predate.
    pub epochs: ProjectedEpochs,
    /// The organization the workspace belongs to.
    ///
    /// The account epoch is keyed by organization, so the floors cannot be bound
    /// to their subjects without it. Taking it from the placement rather than
    /// from the assertion is deliberate: the assertion is what is being checked.
    pub organization_id: Uuid,
    /// Whether paid work is admitted.
    pub account_state: AccountState,
    /// The region the workspace is placed in.
    pub region: Region,
}

/// Why the regional projection could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
    /// The projection has no record of this credential.
    #[error("credential is not projected in this region")]
    Unknown,
    /// The projection could not be read.
    #[error("account state is unavailable")]
    Unavailable,
}

/// The clock the edge stamps a request with.
pub trait EdgeClock: Send + Sync + 'static {
    /// The current instant.
    fn now(&self) -> Timestamp;
}

/// The system clock, used by every deployed composition.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl EdgeClock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
            .unwrap_or_else(|_| Timestamp::from_unix_millis(0).expect("the epoch is in range"))
    }
}

/// The plane-fixed settings an edge is bound to at startup.
///
/// One value rather than five arguments: plane, audience, region, cache budget
/// and effective limits are all resolved from configuration together and are
/// never changed afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeBinding {
    /// The plane this process belongs to.
    ///
    /// Part of the audience the envelope binds, so a `dev` assertion can never
    /// admit a `prd` request even with the same key material.
    pub plane: Plane,
    /// The one service that may accept this deployable's assertions.
    pub audience: AssertionAudience,
    /// The region this process is pinned to.
    pub region: Region,
    /// The assertion cache byte budget.
    pub cache_budget_bytes: usize,
}

impl EdgeBinding {
    /// The exact audience every assertion this edge admits must name.
    #[must_use]
    pub const fn audience(&self) -> Audience {
        Audience {
            plane: self.plane,
            region: self.region,
            service: self.audience,
        }
    }
}

/// The shared fail-closed regional edge.
pub struct RegionalEdge<S, P, L, C> {
    assertions: VerifyingAssertionCache<S>,
    projection: P,
    limit_resolver: L,
    clock: C,
    region: Region,
}

impl<S, P, L, C> RegionalEdge<S, P, L, C>
where
    S: AssertionSource,
    P: ProjectionReader,
    L: LimitResolver,
    C: EdgeClock,
{
    /// Binds an edge to its verified assertion cache and regional projection.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::CacheBudget`] when the assertion cache budget
    /// cannot hold one entry.
    pub fn new(
        source: S,
        keys: VerificationKeySet,
        projection: P,
        limit_resolver: L,
        clock: C,
        binding: EdgeBinding,
    ) -> Result<Self, AuthFailure> {
        Ok(Self {
            assertions: VerifyingAssertionCache::new(
                source,
                keys,
                binding.audience(),
                binding.cache_budget_bytes,
            )?,
            projection,
            limit_resolver,
            clock,
            region: binding.region,
        })
    }

    /// Re-checks mutable regional authorization facts for a long-lived request.
    ///
    /// This deliberately uses the key identity retained in the principal, not
    /// the credential plaintext. Revocation, workspace/account epoch advance,
    /// pause, ownership drift, and placement change all terminate the lease;
    /// an unavailable projection fails closed.
    ///
    /// # Errors
    ///
    /// Returns the public typed authorization failure that should terminate the
    /// already-open transport.
    pub async fn revalidate(&self, auth: &RegionalAuthorization) -> Result<(), WireError> {
        let PrincipalScope::WorkspaceKey {
            key,
            workspace,
            organization,
        } = auth.principal
        else {
            return Err(WireError::new(ErrorCode::Unauthenticated));
        };
        let key_floor = self
            .projection
            .project_key(key)
            .await
            .map_err(projection_error)?;
        let projected = self
            .projection
            .placement(workspace)
            .await
            .map_err(projection_error)?;
        let organization_raw = Uuid::from_bytes(*organization.uuid7().as_bytes());
        let stale = key_floor.key.get() > auth.epochs.key
            || projected.epochs.key.get() > auth.epochs.key
            || projected.epochs.workspace.get() > auth.epochs.workspace
            || projected.epochs.account.get() > auth.epochs.account;
        if stale || projected.organization_id != organization_raw {
            return Err(WireError::new(ErrorCode::TokenRevoked));
        }
        if projected.region != self.region || auth.placement != self.region {
            return Err(WireError::new(ErrorCode::WrongWorkspaceRegion));
        }
        if projected.account_state == AccountState::Paused {
            return Err(WireError::new(ErrorCode::AccountPaused));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl<S, P, L, C> EdgeAdmission for RegionalEdge<S, P, L, C>
where
    S: AssertionSource,
    P: ProjectionReader,
    L: LimitResolver,
    C: EdgeClock,
{
    async fn admit(&self, request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError> {
        let descriptor = route(request.route);
        let now = self.clock.now();

        // 1–2: a credential must be present and well formed.
        let credential = presented(request.headers)?;

        // 3: the credential's own revocation floor is consulted before the
        // assertion, so a revoked credential loses even while its assertion is
        // still inside its 30-second lifetime.
        let credential_floor = self
            .projection
            .project(&credential)
            .await
            .map_err(projection_error)?;

        // 4: verify the credential-bound envelope against that floor. Only the
        // key subject can be checked yet — the workspace an assertion names is a
        // fact the assertion itself carries, and an unverified claim must not
        // choose which projection row is read. `CredentialFloors` therefore
        // defers the other subjects rather than admitting them, and stage 7
        // below applies all of them on every request, cache hit or not.
        let verified = self
            .assertions
            .resolve(
                &credential,
                &CredentialFloors::new(credential_floor.key, credential.key_id_raw()),
                now,
            )
            .await
            .map_err(auth_error)?;

        // 5: a regional edge accepts one principal kind. An envelope for a person
        // carries `user` and `membership` subjects this region projects nothing
        // for, so admitting one would be admitting a credential whose revocation
        // cannot be observed here.
        if verified.claims.principal_kind != PrincipalKind::WorkspaceKey {
            return Err(WireError::new(ErrorCode::Unauthenticated));
        }

        // 6: immutable placement. The assertion names a workspace; the regional
        // projection is what decides whether that workspace lives here, what its
        // current floors are, and whether it is paused. None of those three come
        // from the assertion, because all three can change inside its lifetime.
        let workspace = workspace_id(verified.claims.workspace_id)?;
        let projected = self
            .projection
            .placement(workspace)
            .await
            .map_err(projection_error)?;
        if projected.region != self.region || verified.claims.audience.region != self.region {
            return Err(WireError::new(ErrorCode::WrongWorkspaceRegion));
        }
        // 7: the full subject-bound epoch check, now that every subject id is
        // known. The floors are re-applied on every request, so a revocation or a
        // pause published a moment ago beats a cached assertion.
        let floors = RegionalFloors::new(
            projected.epochs,
            credential.key_id_raw(),
            verified.claims.workspace_id,
            projected.organization_id,
        );
        if verified.claims.organization_id != projected.organization_id
            || !floors.admits(&verified.claims)
        {
            return Err(WireError::new(ErrorCode::Unauthenticated));
        }

        // 8: the scope the table declares for this route.
        if let Some(required) = descriptor.required_scope
            && !verified.claims.scopes.contains(required)
        {
            return Err(WireError::new(ErrorCode::InsufficientScope));
        }

        // 9: the pause gate, exempt only where the table says so. The assertion
        // carries the account state it was minted under and the placement carries
        // the current one; either saying "paused" pauses, because the two differ
        // only when one of them is stale and the safe reading of a stale state is
        // the restrictive one.
        let paused = projected.account_state == AccountState::Paused
            || verified.claims.account_state
                == aex_identity_domain::assertion::AssertedAccountState::PausedTopUpRequired;
        if paused && !descriptor.pause_exempt {
            return Err(WireError::new(ErrorCode::AccountPaused));
        }

        // 10: strongly fence the complete effective-limit revision, then apply
        // the body bound before anything parses the body.
        let limits = self
            .limit_resolver
            .resolve(workspace)
            .await
            .map_err(limit_projection_error)?;
        let body_limit = match descriptor.body_class {
            BodyClass::None => None,
            BodyClass::AexJson => Some(limits.json_body_bytes),
            BodyClass::Otlp => Some(limits.otlp_body_bytes),
        };
        if body_limit.is_some_and(|limit| request.body.len() > limit) {
            return Err(WireError::new(ErrorCode::PayloadTooLarge));
        }

        // 11: replay identity, strict in both directions.
        let (idempotency, durable) = replay_identity(descriptor.idempotency, request, workspace)?;

        Ok(RequestContext {
            request_id: request.request_id.clone(),
            route: request.route,
            auth: authorization(&credential, &verified, workspace, projected)?,
            limits,
            operation_id: durable,
            idempotency,
            if_match: if_match(request.headers)?,
            received_at: offset(now),
        })
    }
}

fn limit_projection_error(_failure: LimitProjectionError) -> WireError {
    WireError::new(ErrorCode::AccountStateUnavailable)
}

fn authorization(
    credential: &PresentedCredential,
    verified: &VerifiedAuthorization,
    workspace: WorkspaceId,
    projected: ProjectedState,
) -> Result<RegionalAuthorization, WireError> {
    let claims = &verified.claims;
    let organization = prefixed::<OrganizationId>(claims.organization_id)?;
    Ok(RegionalAuthorization {
        principal: PrincipalScope::WorkspaceKey {
            key: credential.key_id(),
            workspace,
            organization,
        },
        credential_binding: verified.credential_binding,
        organization_id: organization,
        workspace_id: workspace,
        placement: claims.audience.region,
        // Effective scopes were computed centrally and are never re-derived here.
        scopes: claims.scopes.to_wire(),
        account_state: projected.account_state,
        epochs: epochs(claims),
        issued_at: instant(claims.issued_at_ms),
        expires_at: instant(claims.expires_at_ms),
    })
}

/// Projects the envelope's subject slots onto the named record a handler reads.
///
/// A subject the assertion did not carry stays zero, which is what "no
/// revocation is known for this subject" means everywhere else.
fn epochs(claims: &aex_identity_domain::assertion::AssertionClaims) -> AuthorizationEpochs {
    let mut epochs = AuthorizationEpochs::default();
    for slot in claims.epochs.used() {
        let value = slot.epoch.get();
        match slot.kind {
            EpochSubjectKind::Key => epochs.key = value,
            EpochSubjectKind::Membership => epochs.membership = value,
            EpochSubjectKind::Workspace => epochs.workspace = value,
            EpochSubjectKind::Account => epochs.account = value,
            EpochSubjectKind::Empty | EpochSubjectKind::User => {}
        }
    }
    epochs
}

/// The workspace the assertion names, as the public identifier type.
fn workspace_id(raw: Uuid) -> Result<WorkspaceId, WireError> {
    prefixed(raw)
}

/// A signed raw payload rendered as its prefixed public identifier.
///
/// The envelope carries 16 raw bytes; the public wire carries a prefixed
/// `UUIDv7`. A payload that is not a `UUIDv7` cannot have been minted by the
/// control plane, so it is a refusal rather than a value to render anyway.
fn prefixed<I: aex_wire::ids::PrefixedId>(raw: Uuid) -> Result<I, WireError> {
    Uuid7::from_bytes(*raw.as_bytes())
        .map(I::from_uuid7)
        .map_err(|_| WireError::new(ErrorCode::Unauthenticated))
}

fn instant(millis: u64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

fn offset(value: Timestamp) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(value.unix_millis()) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

fn presented(headers: &HeaderMap) -> Result<PresentedCredential, WireError> {
    let raw = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| WireError::new(ErrorCode::Unauthenticated))?;
    PresentedCredential::new(raw.as_bytes().to_vec())
        .map_err(|_| WireError::new(ErrorCode::Unauthenticated))
}

/// A projection that has no record of a credential or workspace refuses the
/// request as unauthenticated; one that could not answer refuses it as
/// unavailable. Neither becomes an optimistic admission.
fn projection_error(failure: ProjectionError) -> WireError {
    match failure {
        ProjectionError::Unknown => WireError::new(ErrorCode::Unauthenticated),
        ProjectionError::Unavailable => WireError::new(ErrorCode::AccountStateUnavailable),
    }
}

/// Maps a verification failure onto its public code.
///
/// Every cryptographic and semantic refusal collapses into one
/// `unauthenticated`, so a caller cannot distinguish "wrong key" from "expired"
/// from "revoked" by probing. Only the conditions a caller should retry are
/// separated out.
///
/// The match is exhaustive rather than defaulted: a new failure that nobody
/// classified would otherwise reach a caller as `unauthenticated`, which tells a
/// customer their credential is bad when the truth may be that this process was
/// busy. A new variant breaks this function instead.
fn auth_error(failure: AuthFailure) -> WireError {
    match failure {
        // Transient and retryable: identity could not be established right now,
        // and nothing was decided about the credential itself.
        AuthFailure::SourceUnavailable
        | AuthFailure::FlightCapacity
        | AuthFailure::FlightCancelled => WireError::new(ErrorCode::AuthenticationUnavailable),
        AuthFailure::AccountStateUnavailable => WireError::new(ErrorCode::AccountStateUnavailable),
        AuthFailure::CacheBudget => WireError::new(ErrorCode::InternalError),
        AuthFailure::MalformedCredential
        | AuthFailure::MalformedAssertion
        | AuthFailure::Refused
        | AuthFailure::Verification(_) => WireError::new(ErrorCode::Unauthenticated),
    }
}

fn if_match(headers: &HeaderMap) -> Result<Option<ETag>, WireError> {
    match headers.get(http::header::IF_MATCH) {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|text| ETag::parse(text).ok())
            .map(Some)
            .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest)),
    }
}

type ReplayIdentity = (
    Option<crate::idempotency::IdempotencyIdentity>,
    Option<aex_wire::ids::OperationId>,
);

/// Strict in both directions: a route that declares a replay identity refuses a
/// request without one, and a route that declares none refuses a request that
/// supplies one. A silently ignored `Idempotency-Key` is worse than a rejected
/// request, because the caller believes it has a guarantee it does not have.
fn replay_identity(
    kind: IdempotencyKind,
    request: &AdmissionRequest<'_>,
    workspace: WorkspaceId,
) -> Result<ReplayIdentity, WireError> {
    let supplied_key = request
        .headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok());
    let supplied_operation = request.headers.get("aex-operation-id").is_some();
    match kind {
        IdempotencyKind::None => {
            if supplied_key.is_some() || supplied_operation {
                return Err(WireError::new(ErrorCode::InvalidRequest).with_message(
                    "this route declares no replay identity and refuses one".to_owned(),
                ));
            }
            Ok((None, None))
        }
        IdempotencyKind::IdempotencyKey => {
            let raw = supplied_key.ok_or_else(|| {
                WireError::new(ErrorCode::InvalidRequest)
                    .with_message("this route requires an `Idempotency-Key`".to_owned())
            })?;
            let key = IdempotencyKey::parse(raw)
                .map_err(|_| WireError::new(ErrorCode::InvalidRequest))?;
            let canonical = canonical_intent(request.body)?;
            let principal = principal_key(request);
            let organization = workspace.to_string();
            let context = IdentityContext {
                principal: &principal,
                organization: &organization,
                workspace,
                route: request.route,
                method: request.method,
            };
            Ok((Some(identity(&context, &key, &canonical)), None))
        }
        IdempotencyKind::OperationId => {
            let durable = operation_id(request.headers)
                .map_err(|_| WireError::new(ErrorCode::InvalidRequest))?;
            Ok((None, Some(durable)))
        }
    }
}

fn principal_key(request: &AdmissionRequest<'_>) -> String {
    let mut digest = sha2::Sha256::new();
    digest.update(request.route.as_str().as_bytes());
    digest.update(
        request
            .headers
            .get(http::header::AUTHORIZATION)
            .map(http::HeaderValue::as_bytes)
            .unwrap_or_default(),
    );
    hex_lower(&digest.finalize())
}

fn canonical_intent(body: &[u8]) -> Result<Vec<u8>, WireError> {
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| WireError::new(ErrorCode::InvalidRequest))?;
    aex_wire::canonical::to_jcs_bytes(&value).map_err(|_| WireError::new(ErrorCode::InvalidRequest))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod capacity_error_tests {
    use aex_wire::error::ErrorCode;

    use super::limit_projection_error;
    use crate::capacity::LimitProjectionError;

    #[test]
    fn every_capacity_projection_failure_is_a_retryable_unavailable_response() {
        for failure in [
            LimitProjectionError::Unavailable,
            LimitProjectionError::Incomplete,
            LimitProjectionError::InvalidCapacity,
        ] {
            assert_eq!(
                limit_projection_error(failure).code,
                ErrorCode::AccountStateUnavailable
            );
        }
    }
}
