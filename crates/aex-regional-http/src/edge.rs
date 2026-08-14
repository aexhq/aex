//! The shared regional edge: the precedence stages every finite regional
//! deployable runs before the generated dispatcher sees a request.
//!
//! One implementation, three deployables. The stage order is
//! [`crate::router::EDGE_PRECEDENCE`] and each stage's decision comes from the
//! generated route descriptor, never from the handler: `required_scope`,
//! `pause_exempt` and `idempotency` are table facts.
//!
//! # The ten stages, after the central assertion was removed
//!
//! | # | What it decides |
//! | --- | --- |
//! | 1–2 | a credential is present, well formed, and pinned to this region |
//! | 3 | one reconciled projection read; a revoked key stops here |
//! | 4 | the peppered MAC of the presented secret against the projected verifier |
//! | 5 | the key row names *this* edge's audience |
//! | 6 | the placement the control plane published is this region |
//! | 7 | the scope the route table declares |
//! | 8 | the pause gate, exempt only where the table says so |
//! | 9 | the body bound, before anything parses a body |
//! | 10 | replay identity, strict in both directions |
//!
//! Still ten, but stages 4–6 are not the ones they were. Stage 4 was an Ed25519
//! envelope verification against a cached 30-second assertion and is now the
//! in-process credential check. Old stage 5 (principal kind, and the assertion's
//! workspace against the key row's) is gone: there is no envelope to carry a
//! principal kind, and the workspace agreement is now settled inside
//! `read_admission_snapshot`'s own reconciliation, before this function sees a
//! row. Old stage 6 — every carried epoch against the regional floor — is gone
//! for the same reason: nothing carries a claimed epoch any more, so the floors
//! are not a check on an assertion but the answer itself. The region pin moved
//! down into 6 and the audience check took 5, so the count and the numbering
//! stay honest.
//!
//! The epochs still matter, but at a different boundary:
//! [`RegionalEdge::revalidate`] compares a long-lived lease's retained floors
//! against the current projection, which is what ends an open socket when a
//! revocation lands after it was opened.

use aex_internal_contracts::assertion::{AssertionAudience, AudienceSet};
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::idempotency::{IdempotencyKey, IdempotencyKind, PrincipalScope};
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::routes::{BodyClass, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{ETag, Region, Timestamp};
use http::HeaderMap;
use sha2::Digest as _;
use uuid::Uuid;

use crate::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use crate::credential::{
    AuthFailure, PepperRing, PresentedCredential, ProjectedEpochs, StoredVerifier,
};
use crate::idempotency::{IdentityContext, identity, operation_id};
use crate::mount::{AdmissionRequest, EdgeAdmission};

/// The regional projection that *is* the authorization answer.
///
/// The credential names its own workspace as well as its own key, so every row
/// admission needs is addressable before anything has been verified. All of them
/// are therefore read in one reconciled snapshot rather than at the several
/// points in the pipeline where they used to become knowable. Reading them
/// separately was not merely slower: a revocation landing between two point
/// reads could be missed by both.
///
/// The read happens on **every** request and is never cached. Nothing is held
/// between requests any more — there is no assertion and no 30-second window —
/// so a revoked key, a paused account or an advanced revocation epoch takes
/// effect on the next request that reads the projection.
#[async_trait::async_trait]
pub trait ProjectionReader: Send + Sync + 'static {
    /// Reads the whole admission state for a known key and workspace identity.
    ///
    /// Long-lived transports retain the projected identities rather than the
    /// credential plaintext, so a socket revalidates through the same operation
    /// without holding a bearer token for its lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Unknown`] when this region holds no usable
    /// record of that key and workspace — an absent row and a row that
    /// contradicts its siblings are the same answer — and
    /// [`ProjectionError::Unavailable`] when it could not answer at all. Neither
    /// is ever downgraded to an optimistic admission.
    async fn snapshot(
        &self,
        key: ApiKeyId,
        workspace: WorkspaceId,
    ) -> Result<ProjectedState, ProjectionError>;
}

/// What the regional projection says about one key and its workspace right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedState {
    /// The monotonic revocation floors, as published.
    pub epochs: ProjectedEpochs,
    /// The workspace the key row itself names.
    ///
    /// Taken from the key row rather than from the credential, because the
    /// credential's workspace segment is a claim that selected which rows were
    /// read. This is the answer the store gave.
    pub workspace_id: WorkspaceId,
    /// The organization the workspace belongs to.
    ///
    /// The account epoch is keyed by organization, so a long-lived lease cannot
    /// be re-checked against its subject without it.
    pub organization_id: Uuid,
    /// Whether the key itself has been revoked.
    pub key_revoked: bool,
    /// The proof a presented credential is checked against.
    pub verifier: StoredVerifier,
    /// The regional edges this key may be presented to.
    pub audiences: AudienceSet,
    /// The effective scopes, computed centrally and never re-derived here.
    pub scopes: ScopeSet,
    /// Whether paid work is admitted.
    pub account_state: AccountState,
    /// The region the workspace is placed in.
    pub region: Region,
    /// The hot admission ceilings, at the capacity revision they were cut at.
    pub limits: EffectiveLimits,
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
/// One value rather than two arguments: audience and region are resolved from
/// configuration together and are never changed afterwards.
///
/// # Why there is no plane here any more
///
/// The plane used to be part of the assertion audience, so that a `dev`
/// assertion could never admit a `prd` request. With no assertion, plane
/// separation is carried by the two things a request cannot cross: the
/// authorization projection table a process is bound to holds only its own
/// plane's keys, and the pepper ring it loads at cold start is its own plane's
/// secret. A `dev` credential presented to a `prd` edge misses the row, and
/// would fail the MAC even if it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeBinding {
    /// The one audience this deployable accepts a credential for.
    pub audience: AssertionAudience,
    /// The region this process is pinned to.
    pub region: Region,
}

/// The shared fail-closed regional edge.
///
/// Two peers rather than three: the pepper ring resolved at cold start and the
/// regional projection read on every request. There is no central authority
/// port, because there is no longer a request path that leaves the region.
pub struct RegionalEdge<P, C> {
    peppers: PepperRing,
    projection: P,
    clock: C,
    region: Region,
    audience: AssertionAudience,
}

impl<P, C> RegionalEdge<P, C>
where
    P: ProjectionReader,
    C: EdgeClock,
{
    /// Binds an edge to its pepper ring and regional projection.
    ///
    /// Infallible: every input has already been validated by the parser that
    /// produced it, and there is no budget left to get wrong.
    pub const fn new(peppers: PepperRing, projection: P, clock: C, binding: EdgeBinding) -> Self {
        Self {
            peppers,
            projection,
            clock,
            region: binding.region,
            audience: binding.audience,
        }
    }

    /// Re-checks mutable regional authorization facts for a long-lived request.
    ///
    /// This deliberately uses the identities retained in the principal, not the
    /// credential plaintext. Revocation, workspace/account epoch advance, pause,
    /// ownership drift, and placement change all terminate the lease; an
    /// unavailable projection fails closed.
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
        let projected = self
            .projection
            .snapshot(key, workspace)
            .await
            .map_err(projection_error)?;
        let organization_raw = Uuid::from_bytes(*organization.uuid7().as_bytes());
        let stale = projected.key_revoked
            || projected.epochs.key.get() > auth.epochs.key
            || projected.epochs.workspace.get() > auth.epochs.workspace
            || projected.epochs.account.get() > auth.epochs.account;
        if stale
            || projected.organization_id != organization_raw
            || projected.workspace_id != workspace
        {
            return Err(WireError::new(ErrorCode::TokenRevoked));
        }
        if projected.region != self.region || auth.placement != self.region {
            return Err(WireError::new(ErrorCode::WrongWorkspaceRegion));
        }
        // A lease outlives the request that opened it, so the audience is
        // re-read rather than remembered: a key narrowed to no longer authorize
        // this edge must lose its open socket too, not only its next request.
        if !projected.audiences.contains(self.audience) {
            return Err(WireError::new(ErrorCode::TokenRevoked));
        }
        if projected.account_state == AccountState::Paused {
            return Err(WireError::new(ErrorCode::AccountPaused));
        }
        Ok(())
    }

    /// Precedence stages 1–6: the caller is real, and belongs here.
    ///
    /// Split out of [`EdgeAdmission::admit`] because these six answer one
    /// question — is this a genuine credential, for a workspace this region
    /// holds? — while 7–10 are per-route policy applied to a caller already
    /// identified. Every stage here can refuse, and each records which one did
    /// before returning, because the public answers deliberately collapse: four
    /// of these six refusals are published as the same `unauthenticated` a
    /// missing header produces.
    ///
    /// # Errors
    ///
    /// Returns the typed refusal of the first stage that failed.
    async fn authenticate(
        &self,
        request: &AdmissionRequest<'_>,
    ) -> Result<(PresentedCredential, ProjectedState), WireError> {
        // 1–2: a credential must be present and well formed. It names its own
        // key, workspace and region, which is what makes stage 3 addressable.
        let credential = presented(request.headers)
            .map_err(|error| refused(request, None, 1, "credential_absent_or_malformed", error))?;
        if credential.region() != self.region {
            return Err(refused(
                request,
                Some(&credential),
                2,
                "token_region_is_not_this_edge",
                WireError::new(ErrorCode::WrongWorkspaceRegion),
            ));
        }

        // 3: one reconciled read of the key row and placement. It runs before
        // the credential is checked, so a revoked key costs no MAC at all, and
        // the answers cannot disagree about an identity the way separate point
        // reads could. Request ceilings are the validated static binding held
        // by the projection adapter, not another identity row.
        //
        // The workspace here is the credential's *claim*. It only selects which
        // rows are read; `read_admission_snapshot` refuses any set whose
        // identities disagree, so a forged segment costs one miss.
        let projected = self
            .projection
            .snapshot(credential.key_id(), credential.workspace_id())
            .await
            .map_err(|failure| {
                refused(
                    request,
                    Some(&credential),
                    3,
                    projection_reason(failure),
                    projection_error(failure),
                )
            })?;
        if projected.key_revoked {
            return Err(refused(
                request,
                Some(&credential),
                3,
                "key_revoked",
                WireError::new(ErrorCode::TokenRevoked),
            ));
        }

        // 4: authenticate. The peppered MAC of the presented secret must equal
        // the verifier the control plane replicated onto this row, under the
        // pepper version the row names. This is the stage that used to be a
        // synchronous `central-authz` invoke and a 30-second cached envelope.
        self.peppers
            .admits(&credential, &projected.verifier)
            .map_err(|failure| {
                refused(
                    request,
                    Some(&credential),
                    4,
                    failure.reason(),
                    auth_error(failure),
                )
            })?;

        // 5: the key row must name *this* edge. One process can serve two
        // audiences over one table — the unary and stream halves of
        // `session-stream-api` do — so without this a credential admitted at one
        // would be admitted at the other. The assertion used to carry the
        // audience; the row carries it now.
        if !projected.audiences.contains(self.audience) {
            return Err(refused(
                request,
                Some(&credential),
                5,
                AuthFailure::AudienceMismatch.reason(),
                auth_error(AuthFailure::AudienceMismatch),
            ));
        }

        // 6: the region pin, from the row rather than from the token. The
        // credential's own segment was checked at stage 1–2 and is a claim; this
        // is the placement the control plane published.
        if projected.region != self.region {
            return Err(refused(
                request,
                Some(&credential),
                6,
                "projected_placement_is_not_this_region",
                WireError::new(ErrorCode::WrongWorkspaceRegion),
            ));
        }

        Ok((credential, projected))
    }
}

#[async_trait::async_trait]
impl<P, C> EdgeAdmission for RegionalEdge<P, C>
where
    P: ProjectionReader,
    C: EdgeClock,
{
    async fn admit(&self, request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError> {
        let descriptor = route(request.route);
        let now = self.clock.now();

        // 1–6: identify the caller and prove the credential. See
        // `RegionalEdge::authenticate`.
        let (credential, projected) = self.authenticate(request).await?;
        let workspace = credential.workspace_id();

        // 7: the scope the table declares for this route, against the effective
        // scopes computed centrally and carried on the row.
        if let Some(required) = descriptor.required_scope
            && !projected.scopes.contains(required)
        {
            return Err(refused(
                request,
                Some(&credential),
                7,
                "insufficient_scope",
                WireError::new(ErrorCode::InsufficientScope),
            ));
        }

        // 8: the pause gate, exempt only where the table says so. There is one
        // account state now — the placement's — rather than a projected one and
        // an asserted one that could disagree.
        if projected.account_state == AccountState::Paused && !descriptor.pause_exempt {
            return Err(refused(
                request,
                Some(&credential),
                8,
                "account_paused",
                WireError::new(ErrorCode::AccountPaused),
            ));
        }

        // 9: the body bound, from the same snapshot, applied before anything
        // parses the body.
        let limits = projected.limits;
        let body_limit = match descriptor.body_class {
            BodyClass::None => None,
            BodyClass::AexJson => Some(limits.json_body_bytes),
            BodyClass::Otlp => Some(limits.otlp_body_bytes),
            BodyClass::Binary => Some(aex_wire::dispatch::RequestLimits::DEFAULT_BINARY_BODY_BYTES),
        };
        if body_limit.is_some_and(|limit| request.body.len() > limit) {
            return Err(refused(
                request,
                Some(&credential),
                9,
                "body_exceeds_the_projected_limit",
                WireError::new(ErrorCode::PayloadTooLarge),
            ));
        }

        // 10: replay identity, strict in both directions.
        let (idempotency, durable) = replay_identity(descriptor.idempotency, request, workspace)
            .map_err(|error| {
                refused(
                    request,
                    Some(&credential),
                    10,
                    "replay_identity_refused",
                    error,
                )
            })?;

        Ok(RequestContext {
            request_id: request.request_id.clone(),
            route: request.route,
            auth: authorization(&credential, workspace, projected)?,
            limits,
            operation_id: durable,
            idempotency,
            if_match: if_match(request.headers)?,
            received_at: offset(now),
        })
    }
}

fn authorization(
    credential: &PresentedCredential,
    workspace: WorkspaceId,
    projected: ProjectedState,
) -> Result<RegionalAuthorization, WireError> {
    let organization = prefixed::<OrganizationId>(projected.organization_id)?;
    Ok(RegionalAuthorization {
        principal: PrincipalScope::WorkspaceKey {
            key: credential.key_id(),
            workspace,
            organization,
        },
        credential_binding: credential.binding(),
        organization_id: organization,
        workspace_id: workspace,
        // The placement the store gave, which stage 6 already proved is this
        // region's. Taking it from the row rather than from the credential means
        // a handler is told where the workspace *is*, not where its token claims
        // it is.
        placement: projected.region,
        // Effective scopes were computed centrally and are never re-derived here.
        scopes: projected.scopes,
        account_state: projected.account_state,
        epochs: epochs(&projected.epochs),
    })
}

/// Projects the three floors this region publishes onto the named record a
/// handler reads and a long-lived lease is re-checked against.
///
/// `membership` stays zero: a region holds no membership row, so "no revocation
/// is known for this subject" is the only honest value, and it is the same one
/// the envelope's unused slots used to produce.
const fn epochs(projected: &ProjectedEpochs) -> AuthorizationEpochs {
    AuthorizationEpochs {
        key: projected.key.get(),
        membership: 0,
        workspace: projected.workspace.get(),
        account: projected.account.get(),
    }
}

/// A raw payload rendered as its prefixed public identifier.
///
/// The projection carries 16 raw bytes; the public wire carries a prefixed
/// `UUIDv7`. A payload that is not a `UUIDv7` cannot have been minted by the
/// control plane, so it is a refusal rather than a value to render anyway.
fn prefixed<I: aex_wire::ids::PrefixedId>(raw: Uuid) -> Result<I, WireError> {
    Uuid7::from_bytes(*raw.as_bytes())
        .map(I::from_uuid7)
        .map_err(|_| WireError::new(ErrorCode::Unauthenticated))
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

/// Maps an authentication failure onto its public code.
///
/// Every refusal about the credential itself collapses into one
/// `unauthenticated`, so a caller cannot distinguish "unknown key" from "wrong
/// secret" from "wrong edge" by probing. Only the condition a caller should
/// retry — this process cannot check the credential at all — is separated out,
/// and it is a `503`.
///
/// The match is exhaustive rather than defaulted: a new failure that nobody
/// classified would otherwise reach a caller as `unauthenticated`, which tells a
/// customer their credential is bad when the truth may be that we lost a pepper.
/// A new variant breaks this function instead.
fn auth_error(failure: AuthFailure) -> WireError {
    match failure {
        // Transient and retryable: the credential is unverifiable rather than
        // invalid, and nothing at all was decided about it.
        AuthFailure::UnknownPepper { .. } => WireError::new(ErrorCode::AuthenticationUnavailable),
        AuthFailure::MalformedCredential
        | AuthFailure::VerifierMismatch
        | AuthFailure::AudienceMismatch => WireError::new(ErrorCode::Unauthenticated),
    }
}

/// Records which stage refused a request, then returns that refusal unchanged.
///
/// # Why this exists
///
/// [`auth_error`] deliberately collapses "unknown key", "wrong secret" and
/// "wrong edge" into one `unauthenticated`, and [`projection_error`] maps a
/// missing row onto the same code, so that a caller cannot learn which by
/// probing. That property is worth keeping and this does not weaken it: the
/// distinction goes to the process log, which the caller never sees.
///
/// It exists because the collapse had a second audience nobody intended — us.
/// Every stage below discarded its reason the instant it decided, so a refusal
/// left no trace anywhere: not a log line, not a counter, not a header. The
/// release-evidence lane failed twelve consecutive times on a `401` from this
/// exact path and no operator could tell which of eight stages produced it
/// without attaching to a live task. `tracing` was already a declared
/// dependency of this crate and was called in precisely zero places.
///
/// # What may be recorded here
///
/// Only identities the caller already holds and the platform already publishes:
/// the key id and workspace id, both public prefixed identifiers, and the
/// request id that the error envelope returns to the caller anyway — that last
/// one is what joins a customer's support ticket to this line. The credential
/// plaintext and its digest are not parameters of this function, so no future
/// edit can reach them from here.
fn refused(
    request: &AdmissionRequest<'_>,
    credential: Option<&PresentedCredential>,
    stage: u8,
    reason: &'static str,
    error: WireError,
) -> WireError {
    // Rendered rather than skipped when the credential never parsed, so the
    // field set is the same shape on every line and a log query does not have
    // to special-case the earliest stage.
    let (key_id, workspace_id) = credential.map_or_else(
        || ("<unparsed>".to_owned(), "<unparsed>".to_owned()),
        |credential| {
            (
                credential.key_id().to_string(),
                credential.workspace_id().to_string(),
            )
        },
    );
    tracing::warn!(
        target: "aex.regional.admission",
        stage = u64::from(stage),
        reason,
        code = error.code.as_str(),
        route = request.route.as_str(),
        request_id = %request.request_id,
        key_id,
        workspace_id,
        "regional admission refused the request"
    );
    error
}

/// The log-safe name for a projection that could not answer.
///
/// Paired with [`projection_error`] rather than folded into it because the two
/// have different audiences: that one picks the code the caller sees, this one
/// picks the token an operator greps for. `Unknown` is by far the most valuable
/// line this module emits — it is the one that says the region has no row for a
/// key that exists centrally, which is a replication failure and not a bad
/// credential, and the wire cannot say so without becoming an oracle.
const fn projection_reason(failure: ProjectionError) -> &'static str {
    match failure {
        ProjectionError::Unknown => "no_projected_row_for_key_and_workspace",
        ProjectionError::Unavailable => "projection_unavailable",
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
mod projection_error_tests {
    use std::collections::BTreeSet;

    use aex_wire::error::ErrorCode;

    use super::{auth_error, projection_error, projection_reason};
    use crate::credential::AuthFailure;
    use crate::edge::ProjectionError;

    #[test]
    fn an_unknown_snapshot_refuses_and_an_unavailable_one_is_retryable() {
        assert_eq!(
            projection_error(ProjectionError::Unknown).code,
            ErrorCode::Unauthenticated
        );
        assert_eq!(
            projection_error(ProjectionError::Unavailable).code,
            ErrorCode::AccountStateUnavailable
        );
    }

    /// The whole point of the log line is that it says something the wire will
    /// not, so two stages that share a public code must not share a reason.
    #[test]
    fn every_refusal_that_collapses_onto_one_code_still_has_its_own_reason() {
        let failures = [
            AuthFailure::MalformedCredential,
            AuthFailure::VerifierMismatch,
            AuthFailure::AudienceMismatch,
            AuthFailure::UnknownPepper { version: 1 },
        ];

        let unauthenticated = failures
            .iter()
            .filter(|failure| auth_error(**failure).code == ErrorCode::Unauthenticated)
            .count();
        assert_eq!(
            unauthenticated, 3,
            "three distinct causes are published as one `unauthenticated`, which is why the log \
             has to separate them"
        );

        let reasons = failures
            .iter()
            .map(|failure| failure.reason())
            .chain([
                projection_reason(ProjectionError::Unknown),
                projection_reason(ProjectionError::Unavailable),
            ])
            .collect::<BTreeSet<_>>();
        assert_eq!(
            reasons.len(),
            6,
            "a duplicated reason makes two different failures indistinguishable in the log, \
             which is the defect this exists to fix"
        );
    }

    /// A reason is a grep target across a region's logs, so it may not carry a
    /// space, a quote or any part of a credential.
    #[test]
    fn reasons_are_stable_greppable_tokens() {
        for reason in [
            AuthFailure::MalformedCredential.reason(),
            AuthFailure::VerifierMismatch.reason(),
            AuthFailure::AudienceMismatch.reason(),
            AuthFailure::UnknownPepper { version: 7 }.reason(),
            projection_reason(ProjectionError::Unknown),
            projection_reason(ProjectionError::Unavailable),
        ] {
            assert!(!reason.is_empty());
            assert!(
                reason
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "`{reason}` is not a lowercase underscore token"
            );
        }
    }

    /// Logging must not have moved a single public answer. Recording why a
    /// stage refused is worth doing only because it changes nothing the caller
    /// can observe; a drift here would turn the log into an oracle.
    #[test]
    fn recording_the_reason_did_not_change_any_published_code() {
        assert_eq!(
            auth_error(AuthFailure::UnknownPepper { version: 3 }).code,
            ErrorCode::AuthenticationUnavailable
        );
        for failure in [
            AuthFailure::MalformedCredential,
            AuthFailure::VerifierMismatch,
            AuthFailure::AudienceMismatch,
        ] {
            assert_eq!(auth_error(failure).code, ErrorCode::Unauthenticated);
        }
    }
}
