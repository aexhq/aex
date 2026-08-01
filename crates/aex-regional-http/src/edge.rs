//! The shared regional edge: the precedence stages every finite regional
//! deployable runs before the generated dispatcher sees a request.
//!
//! One implementation, three deployables. The stage order is
//! [`crate::router::EDGE_PRECEDENCE`] and each stage's decision comes from the
//! generated route descriptor, never from the handler: `required_scope`,
//! `pause_exempt` and `idempotency` are table facts.

use aex_internal_contracts::assertion::AssertionAudience;
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::idempotency::{IdempotencyKey, IdempotencyKind};
use aex_wire::ids::WorkspaceId;
use aex_wire::routes::route;
use aex_wire::types::{ETag, Region, Timestamp};
use http::HeaderMap;
use sha2::Digest as _;

use crate::assertion::{
    AssertionSource, AuthFailure, KeyVerifier, PresentedCredential, ProjectedEpochs,
    VerifiedAuthorization, VerifyingAssertionCache,
};
use crate::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
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
/// One value rather than four arguments: audience, region, cache budget and
/// effective limits are all resolved from configuration together and are never
/// changed afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeBinding {
    /// The one service that may accept this deployable's assertions.
    pub audience: AssertionAudience,
    /// The region this process is pinned to.
    pub region: Region,
    /// The assertion cache byte budget.
    pub cache_budget_bytes: usize,
    /// The effective workspace limits this deployable enforces.
    pub limits: EffectiveLimits,
}

/// The shared fail-closed regional edge.
pub struct RegionalEdge<S, V, P, C> {
    assertions: VerifyingAssertionCache<S, V>,
    projection: P,
    clock: C,
    region: Region,
    limits: EffectiveLimits,
}

impl<S, V, P, C> RegionalEdge<S, V, P, C>
where
    S: AssertionSource,
    V: KeyVerifier,
    P: ProjectionReader,
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
        verifier: V,
        projection: P,
        clock: C,
        binding: EdgeBinding,
    ) -> Result<Self, AuthFailure> {
        Ok(Self {
            assertions: VerifyingAssertionCache::new(
                source,
                verifier,
                binding.audience,
                binding.region,
                binding.cache_budget_bytes,
            )?,
            projection,
            clock,
            region: binding.region,
            limits: binding.limits,
        })
    }
}

#[async_trait::async_trait]
impl<S, V, P, C> EdgeAdmission for RegionalEdge<S, V, P, C>
where
    S: AssertionSource,
    V: KeyVerifier,
    P: ProjectionReader,
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

        // 4: verify the credential-bound assertion for this audience and region.
        let verified = self
            .assertions
            .resolve(&credential, credential_floor, now)
            .await
            .map_err(auth_error)?;

        // 5: immutable placement. The assertion names a workspace; the regional
        // projection is what decides whether that workspace lives here, what its
        // current floors are, and whether it is paused. None of those three come
        // from the assertion, because all three can change inside its lifetime.
        if verified.assertion.region != self.region {
            return Err(WireError::new(ErrorCode::WrongWorkspaceRegion));
        }
        let workspace = verified
            .assertion
            .workspace
            .ok_or_else(|| WireError::new(ErrorCode::Forbidden))?;
        let projected = self
            .projection
            .placement(workspace)
            .await
            .map_err(projection_error)?;
        if projected.region != self.region {
            return Err(WireError::new(ErrorCode::WrongWorkspaceRegion));
        }
        if verified.assertion.key_epoch.0 < projected.epochs.key
            || verified.assertion.account_epoch.0 < projected.epochs.account
            || verified.assertion.revocation_epoch.0 < projected.epochs.revocation
        {
            return Err(WireError::new(ErrorCode::Unauthenticated));
        }

        // 6: the scope the table declares for this route.
        if let Some(required) = descriptor.required_scope
            && !verified.assertion.scopes.contains(required)
        {
            return Err(WireError::new(ErrorCode::InsufficientScope));
        }

        // 7: the pause gate, exempt only where the table says so.
        if projected.account_state == AccountState::Paused && !descriptor.pause_exempt {
            return Err(WireError::new(ErrorCode::AccountPaused));
        }

        // 8: the effective body bound, before anything parses the body.
        if request.body.len() > self.limits.json_body_bytes {
            return Err(WireError::new(ErrorCode::PayloadTooLarge));
        }

        // 9: replay identity, strict in both directions.
        let (idempotency, durable) = replay_identity(descriptor.idempotency, request, workspace)?;

        Ok(RequestContext {
            request_id: request.request_id.clone(),
            route: request.route,
            auth: authorization(&verified, workspace, projected),
            limits: self.limits,
            operation_id: durable,
            idempotency,
            if_match: if_match(request.headers)?,
            received_at: offset(now),
        })
    }
}

fn authorization(
    verified: &VerifiedAuthorization,
    workspace: WorkspaceId,
    projected: ProjectedState,
) -> RegionalAuthorization {
    RegionalAuthorization {
        principal: verified.assertion.principal,
        credential_binding: verified.credential_binding,
        organization_id: verified.assertion.organization,
        workspace_id: workspace,
        placement: verified.assertion.region,
        scopes: verified.assertion.scopes.clone(),
        account_state: projected.account_state,
        epochs: AuthorizationEpochs {
            key: verified.assertion.key_epoch.0,
            membership: projected.epochs.revocation,
            workspace: projected.epochs.revocation,
            account: verified.assertion.account_epoch.0,
        },
        issued_at: offset(verified.assertion.issued_at),
        expires_at: offset(verified.assertion.expires_at),
    }
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

fn auth_error(failure: AuthFailure) -> WireError {
    match failure {
        AuthFailure::SourceUnavailable => WireError::new(ErrorCode::AuthenticationUnavailable),
        AuthFailure::CacheBudget => WireError::new(ErrorCode::InternalError),
        _ => WireError::new(ErrorCode::Unauthenticated),
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
