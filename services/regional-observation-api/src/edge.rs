//! The authenticated request edge.
//!
//! `aex-regional-http` owns the verification algebra — the credential wrapper,
//! the signed-assertion transport, `verify` and the byte-bounded cache. It
//! deliberately leaves two things to a composition root: the concrete crypto
//! behind [`KeyVerifier`], and the transport behind [`AssertionSource`]. This
//! module supplies both, and nothing else here re-implements a check that crate
//! already owns.
//!
//! Fail-closed is structural: every failure path produces a typed
//! [`aex_wire::error::WireError`], and there is no arm that admits a request
//! whose assertion did not verify.

use std::time::Duration;

use aex_internal_contracts::assertion::{AssertionAudience, AuthorizationAssertion};
use aex_regional_http::assertion::{
    AssertionSource, AuthFailure, KeyVerifier, PresentedCredential, ProjectedEpochs,
    SignedAssertion, VerifyingAssertionCache,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::routes::RouteDescriptor;
use aex_wire::server::{AcceptKind, RequestContext};
use aex_wire::types::{RequestId, Timestamp};
use async_trait::async_trait;
use base64::Engine as _;
use http::HeaderMap;
use serde::{Deserialize, Serialize};

/// The internal path `central-authz` serves the assertion exchange on.
pub const ASSERTION_PATH: &str = "/internal/authz/assertions";

/// How long the edge waits for the central authorization authority.
pub const ASSERTION_TIMEOUT: Duration = Duration::from_millis(1_500);

/// The request body of the internal assertion exchange.
///
/// This is a consumed internal contract, not a public route: the credential
/// never leaves the regional plane except towards the authority that issued it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionRequest<'a> {
    /// The presented credential, verbatim.
    pub credential: &'a str,
    /// Which service will accept the assertion.
    pub audience: AssertionAudience,
    /// The region the caller must be pinned to.
    pub region: aex_wire::types::Region,
}

/// The response body of the internal assertion exchange.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AssertionResponse {
    /// The claim set.
    pub assertion: AuthorizationAssertion,
    /// Which trust anchor signed it.
    pub key_id: String,
    /// The credential binding the signature covers, base64.
    pub credential_binding: String,
    /// The detached signature, base64.
    pub signature: String,
}

/// Ed25519 trust anchors resolved at start-up.
///
/// OD-21 splits signature algorithms by key custody: the 30-second
/// authorization assertion is the one AEX holds directly, so it is Ed25519
/// rather than the `ECDSA_SHA_256` a KMS-held key would force.
pub struct Ed25519Anchors {
    anchors: Vec<(String, Vec<u8>)>,
}

impl std::fmt::Debug for Ed25519Anchors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ed25519Anchors")
            .field("count", &self.anchors.len())
            .finish()
    }
}

impl Ed25519Anchors {
    /// Wraps the configured anchors.
    #[must_use]
    pub fn new(anchors: Vec<(String, Vec<u8>)>) -> Self {
        Self { anchors }
    }

    /// How many anchors are trusted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    /// Whether no anchor is trusted, which start-up already refuses.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }
}

impl KeyVerifier for Ed25519Anchors {
    fn verify(&self, key_id: &str, message: &[u8], signature: &[u8]) -> bool {
        // The key id selects exactly one anchor. Trying every anchor would turn
        // a rotated-out key into a silently accepted one.
        let Some((_, material)) = self
            .anchors
            .iter()
            .find(|(candidate, _)| candidate == key_id)
        else {
            return false;
        };
        aws_lc_rs::signature::UnparsedPublicKey::new(&aws_lc_rs::signature::ED25519, material)
            .verify(message, signature)
            .is_ok()
    }
}

/// The `central-authz` assertion exchange over HTTPS.
#[derive(Debug)]
pub struct HttpAssertionSource {
    client: reqwest::Client,
    endpoint: String,
    audience: AssertionAudience,
    region: aex_wire::types::Region,
}

impl HttpAssertionSource {
    /// Builds the source against a resolved endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::SourceUnavailable`] when the HTTP client itself
    /// cannot be constructed, which is a host condition rather than a request
    /// one and must stop start-up rather than every request.
    pub fn new(
        endpoint: &str,
        audience: AssertionAudience,
        region: aex_wire::types::Region,
    ) -> Result<Self, AuthFailure> {
        let client = reqwest::Client::builder()
            .timeout(ASSERTION_TIMEOUT)
            .build()
            .map_err(|_| AuthFailure::SourceUnavailable)?;
        Ok(Self {
            client,
            endpoint: format!("{}{ASSERTION_PATH}", endpoint.trim_end_matches('/')),
            audience,
            region,
        })
    }
}

#[async_trait]
impl AssertionSource for HttpAssertionSource {
    async fn obtain(
        &self,
        credential: &PresentedCredential,
    ) -> Result<SignedAssertion, AuthFailure> {
        let body = credential.expose(|bytes| {
            let credential =
                std::str::from_utf8(bytes).map_err(|_| AuthFailure::MalformedCredential)?;
            serde_json::to_vec(&AssertionRequest {
                credential,
                audience: self.audience,
                region: self.region,
            })
            .map_err(|_| AuthFailure::MalformedCredential)
        })?;
        let response = self
            .client
            .post(&self.endpoint)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| AuthFailure::SourceUnavailable)?;
        if !response.status().is_success() {
            return Err(AuthFailure::SourceUnavailable);
        }
        let payload: AssertionResponse = response
            .json()
            .await
            .map_err(|_| AuthFailure::MalformedAssertion)?;
        let binding: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(&payload.credential_binding)
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or(AuthFailure::MalformedAssertion)?;
        let signature = base64::engine::general_purpose::STANDARD
            .decode(&payload.signature)
            .map_err(|_| AuthFailure::MalformedAssertion)?;
        SignedAssertion::new(payload.assertion, payload.key_id, binding, signature)
    }
}

/// The composed edge: credential extraction, assertion resolution, and the
/// route-table checks that precede every handler.
pub struct RequestAuthority<S, V> {
    cache: VerifyingAssertionCache<S, V>,
}

impl<S, V> RequestAuthority<S, V>
where
    S: AssertionSource,
    V: KeyVerifier,
{
    /// Composes the edge over a source and a verifier.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::CacheBudget`] when the configured cache budget
    /// cannot hold a single entry.
    pub fn new(
        source: S,
        verifier: V,
        audience: AssertionAudience,
        region: aex_wire::types::Region,
        cache_bytes: usize,
    ) -> Result<Self, AuthFailure> {
        Ok(Self {
            cache: VerifyingAssertionCache::new(source, verifier, audience, region, cache_bytes)?,
        })
    }

    /// Authorizes one request and produces the context the dispatcher needs.
    ///
    /// # Errors
    ///
    /// Returns the typed public failure for every stage: a missing or malformed
    /// credential, an unavailable authority, an assertion that does not verify,
    /// a workspace pinned to another region, or a credential that does not
    /// carry the scope the route declares.
    pub async fn authorize(
        &self,
        descriptor: &RouteDescriptor,
        headers: &HeaderMap,
        now: Timestamp,
    ) -> WireResult<Authorized> {
        let credential = bearer(headers)?;
        let verified = self
            .cache
            .resolve(&credential, ProjectedEpochs::default(), now)
            .await
            .map_err(auth_failure)?;
        let assertion = verified.assertion;
        if let Some(required) = descriptor.required_scope
            && !assertion.scopes.contains(required)
        {
            return Err(WireError::new(ErrorCode::InsufficientScope));
        }
        let workspace = assertion
            .workspace
            .ok_or_else(|| WireError::new(ErrorCode::Forbidden))?;
        Ok(Authorized {
            context: RequestContext {
                request_id: request_id(headers),
                route: descriptor.id,
                principal: assertion.principal,
                granted_scopes: assertion.scopes.clone(),
                idempotency_key: None,
                operation_id: aex_regional_http::idempotency::operation_id(headers).ok(),
                if_match: None,
                accept: AcceptKind::Json,
            },
            organization: assertion.organization,
            workspace,
        })
    }
}

/// What the edge established, beyond what the generated context carries.
///
/// The generated [`RequestContext`] deliberately does not carry the resolved
/// workspace: a central route has no workspace at all. A regional composition
/// keeps the richer record next to it rather than widening the generated type.
#[derive(Debug, Clone)]
pub struct Authorized {
    /// What the dispatcher receives.
    pub context: RequestContext,
    /// The owning organization.
    pub organization: aex_wire::ids::OrganizationId,
    /// The authorized workspace.
    pub workspace: aex_wire::ids::WorkspaceId,
}

/// Extracts the bearer credential.
fn bearer(headers: &HeaderMap) -> WireResult<PresentedCredential> {
    let raw = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| WireError::new(ErrorCode::Unauthenticated))?;
    PresentedCredential::new(raw.as_bytes().to_vec())
        .map_err(|_| WireError::new(ErrorCode::Unauthenticated))
}

/// The diagnostic request id, minted when the edge did not receive one.
fn request_id(headers: &HeaderMap) -> RequestId {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| RequestId::parse(value).ok())
        .unwrap_or_else(mint_request_id)
}

/// Mints a fresh diagnostic request id.
fn mint_request_id() -> RequestId {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    RequestId::parse(&raw).unwrap_or_else(|_| {
        RequestId::parse("00000000000000000000000000000000")
            .expect("a 32-character hexadecimal request id always parses")
    })
}

/// Maps a verification failure onto its public code without leaking a reason a
/// caller could use to distinguish "wrong key" from "expired".
fn auth_failure(failure: AuthFailure) -> WireError {
    match failure {
        AuthFailure::SourceUnavailable => WireError::new(ErrorCode::AuthenticationUnavailable),
        AuthFailure::CacheBudget => WireError::new(ErrorCode::InternalError),
        AuthFailure::Region => WireError::new(ErrorCode::WrongWorkspaceRegion),
        _ => WireError::new(ErrorCode::Unauthenticated),
    }
}

/// The audience every assertion this deployable accepts must carry.
///
/// One deployable, one audience: an assertion minted for the query role must
/// never be accepted by the admission role, and the reverse.
pub const AUDIENCE: AssertionAudience = AssertionAudience::RegionalObservation;

#[cfg(test)]
mod tests {
    use aex_regional_http::assertion::KeyVerifier as _;

    use super::{AUDIENCE, Ed25519Anchors};
    use aex_internal_contracts::assertion::AssertionAudience;

    #[test]
    fn an_unknown_key_id_never_verifies() {
        let anchors = Ed25519Anchors::new(vec![("kid".to_owned(), vec![0u8; 32])]);
        assert!(!anchors.is_empty());
        assert_eq!(anchors.len(), 1);
        assert!(
            !anchors.verify("other", b"message", b"signature"),
            "a key id outside the ring must never verify"
        );
    }

    #[test]
    fn a_known_key_id_with_a_wrong_signature_still_fails() {
        let anchors = Ed25519Anchors::new(vec![("kid".to_owned(), vec![3u8; 32])]);
        assert!(!anchors.verify("kid", b"message", &[0u8; 64]));
    }

    #[test]
    fn this_deployable_accepts_only_the_observation_audience() {
        assert_eq!(AUDIENCE, AssertionAudience::RegionalObservation);
        // An assertion minted for the admission role must never be accepted
        // here, and the reverse.
        for other in [
            AssertionAudience::RegionalOtlp,
            AssertionAudience::RegionalSession,
            AssertionAudience::RegionalSecret,
            AssertionAudience::RegionalStream,
        ] {
            assert_ne!(AUDIENCE, other);
        }
    }
}
