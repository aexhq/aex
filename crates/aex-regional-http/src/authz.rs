//! The concrete peer adapters a regional composition root needs to build an
//! edge: the `central-authz` assertion exchange, the trust anchors that verify
//! its answer, the regional authorization projection, and the cursor signing
//! ring every continuation is minted under.
//!
//! [`crate::edge::RegionalEdge`] deliberately keeps all four as ports. This
//! module is where they stop being ports, and it is the only place in the
//! regional plane that names an AWS client for an authorization concern.
//!
//! # What is read when
//!
//! | Input | When | How often |
//! | --- | --- | --- |
//! | trust anchors ([`ParameterStore::trust_anchors`]) | cold start | once |
//! | cursor signing ring ([`ParameterStore::cursor_key_ring`]) | cold start | once |
//! | the assertion ([`LambdaAssertionSource`]) | per credential | once per 30 s |
//! | the projection ([`RegionalProjection`]) | per request | always |
//!
//! The projection is the only per-request read and it is deliberately never
//! cached: it is what makes a revoked key or a paused account take effect inside
//! the 30-second assertion window rather than after it.

use aex_control_domain::epoch::Epoch;
use aex_identity_domain::assertion::{
    Assertion, MAX_VERIFICATION_KEYS, VerificationKey, VerificationKeySet,
};
use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::assertion::{
    AssertionAudience, AssertionRefusal, AssertionResponse, CredentialDigest, ResolveWorkspaceKey,
};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::projection::AuthorizationProjection;
use aex_wire::ids::WorkspaceId;
use aex_wire::types::Region;
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use uuid::Uuid;
use zeroize::{Zeroize as _, Zeroizing};

use crate::assertion::{AssertionSource, AuthFailure, PresentedCredential, ProjectedEpochs};
use crate::context::AccountState;
use crate::cursor::{CursorError, CursorKey, CursorKeyRing};
use crate::edge::{ProjectedState, ProjectionError, ProjectionReader};

/// The most trust anchors a region will hold at once.
///
/// A rotation needs two — the new key and the one still signing in flight — and
/// the bound is generous enough for a slow rollout. It is the envelope's own
/// bound rather than a second number, so a document this parser accepts is one
/// `VerificationKeySet` can hold.
pub const MAX_TRUST_ANCHORS: usize = MAX_VERIFICATION_KEYS;

/// The largest parameter document this module will decode.
pub const MAX_PARAMETER_BYTES: usize = 64 * 1_024;

/// The largest `central-authz` response this module will decode.
pub const MAX_ASSERTION_RESPONSE_BYTES: usize = 64 * 1_024;

/// An Ed25519 public key is exactly 32 bytes.
const PUBLIC_KEY_BYTES: usize = 32;

/// Why start-up key material was refused.
///
/// Every variant stops the process. There is no arm that degrades to an empty
/// anchor set or a generated key: a regional edge that cannot verify an
/// assertion must not start, because the alternative is a process that accepts
/// nothing while reporting ready, or worse, one that accepts everything.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    /// The parameter could not be read.
    #[error("parameter `{name}` could not be read: {reason}")]
    Unreadable {
        /// Which parameter.
        name: String,
        /// What the service reported.
        reason: String,
    },
    /// The parameter held no value.
    #[error("parameter `{name}` holds no value")]
    Empty {
        /// Which parameter.
        name: String,
    },
    /// The parameter was larger than the decode bound.
    #[error("parameter `{name}` is {found} bytes; at most {MAX_PARAMETER_BYTES} are decoded")]
    TooLarge {
        /// Which parameter.
        name: String,
        /// How large it was.
        found: usize,
    },
    /// The document did not decode.
    #[error("parameter `{name}` is not a valid {kind} document: {reason}")]
    Malformed {
        /// Which parameter.
        name: String,
        /// Which document kind was expected.
        kind: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// The anchor set was empty, which would admit nothing.
    #[error("parameter `{name}` declares no trust anchor")]
    NoAnchors {
        /// Which parameter.
        name: String,
    },
    /// The anchor set was past the bound.
    #[error("parameter `{name}` declares {found} anchors; at most {MAX_TRUST_ANCHORS} are held")]
    TooManyAnchors {
        /// Which parameter.
        name: String,
        /// How many were declared.
        found: usize,
    },
    /// Two anchors or two cursor keys claimed the same identity.
    #[error("parameter `{name}` declares `{key_id}` twice")]
    DuplicateKeyId {
        /// Which parameter.
        name: String,
        /// The repeated identity.
        key_id: String,
    },
    /// A cursor key identity was empty, oversized or carried a control byte.
    #[error("parameter `{name}` declares an unusable key identity")]
    KeyIdentity {
        /// Which parameter.
        name: String,
    },
    /// Key material was not the exact length its algorithm requires.
    #[error("parameter `{name}` declares key `{key_id}` with unusable material")]
    KeyMaterial {
        /// Which parameter.
        name: String,
        /// Which key.
        key_id: String,
    },
    /// The cursor ring itself refused the keys.
    #[error("parameter `{name}` does not describe a usable cursor ring: {source}")]
    CursorRing {
        /// Which parameter.
        name: String,
        /// Why the ring refused them.
        source: CursorError,
    },
}

// ---------------------------------------------------------------------------
// Trust anchors
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AnchorDocument {
    #[allow(
        dead_code,
        reason = "decoded so an unversioned or future document is refused rather than guessed at"
    )]
    schema_version: SchemaVersion,
    keys: Vec<AnchorEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AnchorEntry {
    key_id: Uuid,
    public_key: String,
    not_after_ms: u64,
}

/// Decodes the assertion trust-anchor document held at `AEX_AUTHZ_VERIFY_KEYS_PARAM`.
///
/// ```json
/// { "schemaVersion": 1,
///   "keys": [ { "keyId": "0193f0a1-...", "publicKey": "<43 chars unpadded base64url>",
///               "notAfterMs": 1798761600000 } ] }
/// ```
///
/// The identity is the envelope's `kid`: a raw UUID carried in the header at a
/// fixed offset, not a free string. `notAfterMs` is when this region stops
/// accepting the key, which is what lets a rotated-out key be retired without a
/// redeploy of every regional service.
///
/// OD-21 splits signature algorithms by key custody: the 30-second authorization
/// assertion is the one artifact AEX holds the private key for directly, so it is
/// Ed25519 rather than the `ECDSA_SHA_256` a KMS-held key would force.
///
/// # Errors
///
/// Returns [`TrustError`] for an oversized, malformed, empty, over-long or
/// ambiguous document, or one declaring key material that is not exactly 32
/// bytes. Nothing here degrades: a document this function refuses stops the
/// process, because an edge that cannot verify an assertion must not start.
pub fn parse_trust_anchors(name: &str, document: &str) -> Result<VerificationKeySet, TrustError> {
    if document.len() > MAX_PARAMETER_BYTES {
        return Err(TrustError::TooLarge {
            name: name.to_owned(),
            found: document.len(),
        });
    }
    let decoded: AnchorDocument =
        serde_json::from_str(document).map_err(|error| TrustError::Malformed {
            name: name.to_owned(),
            kind: "trust anchor",
            reason: error.to_string(),
        })?;
    if decoded.keys.is_empty() {
        return Err(TrustError::NoAnchors {
            name: name.to_owned(),
        });
    }
    if decoded.keys.len() > MAX_TRUST_ANCHORS {
        return Err(TrustError::TooManyAnchors {
            name: name.to_owned(),
            found: decoded.keys.len(),
        });
    }
    let mut keys: Vec<VerificationKey> = Vec::with_capacity(decoded.keys.len());
    for entry in decoded.keys {
        if keys.iter().any(|seen| seen.kid.get() == entry.key_id) {
            return Err(TrustError::DuplicateKeyId {
                name: name.to_owned(),
                key_id: entry.key_id.to_string(),
            });
        }
        let public_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&entry.public_key)
            .ok()
            .and_then(|bytes| <[u8; PUBLIC_KEY_BYTES]>::try_from(bytes).ok())
            .ok_or_else(|| TrustError::KeyMaterial {
                name: name.to_owned(),
                key_id: entry.key_id.to_string(),
            })?;
        keys.push(VerificationKey {
            kid: aex_identity_domain::assertion::KeyId::new(entry.key_id),
            public_key,
            not_after_ms: entry.not_after_ms,
        });
    }
    VerificationKeySet::new(keys).map_err(|_| TrustError::TooManyAnchors {
        name: name.to_owned(),
        found: MAX_TRUST_ANCHORS + 1,
    })
}

// ---------------------------------------------------------------------------
// Cursor signing ring
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CursorDocument {
    #[allow(
        dead_code,
        reason = "decoded so an unversioned or future document is refused rather than guessed at"
    )]
    schema_version: SchemaVersion,
    current: CursorEntry,
    #[serde(default)]
    overlap: Vec<CursorEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CursorEntry {
    key_id: String,
    material: String,
}

/// Decodes the cursor signing ring `AEX_CURSOR_SIGNING_KEY_REF` names.
///
/// The reference is a Parameter Store name holding a `SecureString`:
///
/// ```json
/// { "schemaVersion": 1,
///   "current": { "keyId": "2026-08", "material": "<>= 32 bytes, unpadded base64url>" },
///   "overlap": [ { "keyId": "2026-07", "material": "…" } ] }
/// ```
///
/// The first entry is the only one a new cursor is signed under; the overlap
/// entries verify and never sign, which is what stops a ring from continuing to
/// issue under a key that is on its way out.
///
/// # Errors
///
/// Returns [`TrustError`] for an oversized, malformed or ambiguous document, or
/// one whose material is shorter than [`crate::cursor::MIN_KEY_BYTES`].
pub fn parse_cursor_key_ring(name: &str, document: &str) -> Result<CursorKeyRing, TrustError> {
    if document.len() > MAX_PARAMETER_BYTES {
        return Err(TrustError::TooLarge {
            name: name.to_owned(),
            found: document.len(),
        });
    }
    let mut decoded: CursorDocument =
        serde_json::from_str(document).map_err(|error| TrustError::Malformed {
            name: name.to_owned(),
            kind: "cursor signing ring",
            reason: error.to_string(),
        })?;
    let current = cursor_key(name, &mut decoded.current)?;
    let mut overlap = Vec::with_capacity(decoded.overlap.len());
    for entry in &mut decoded.overlap {
        overlap.push(cursor_key(name, entry)?);
    }
    CursorKeyRing::new(current, overlap).map_err(|source| TrustError::CursorRing {
        name: name.to_owned(),
        source,
    })
}

/// Decodes one cursor key, zeroizing the encoded material it consumed.
fn cursor_key(name: &str, entry: &mut CursorEntry) -> Result<CursorKey, TrustError> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&entry.material);
    entry.material.zeroize();
    let mut material = Zeroizing::new(decoded.map_err(|_| TrustError::KeyMaterial {
        name: name.to_owned(),
        key_id: entry.key_id.clone(),
    })?);
    CursorKey::new(entry.key_id.clone(), std::mem::take(&mut *material)).map_err(|source| {
        match source {
            CursorError::WeakKey { .. } => TrustError::KeyMaterial {
                name: name.to_owned(),
                key_id: entry.key_id.clone(),
            },
            _ => TrustError::KeyIdentity {
                name: name.to_owned(),
            },
        }
    })
}

// ---------------------------------------------------------------------------
// Parameter Store
// ---------------------------------------------------------------------------

/// The Parameter Store reader a composition root resolves key material through.
///
/// Both reads happen once, at cold start, before the listener binds. A failure
/// stops the process rather than the request.
#[derive(Debug, Clone)]
pub struct ParameterStore {
    client: aws_sdk_ssm::Client,
}

impl ParameterStore {
    /// Binds the reader to a client.
    #[must_use]
    pub const fn new(client: aws_sdk_ssm::Client) -> Self {
        Self { client }
    }

    /// Reads one decrypted parameter value.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError::Unreadable`] for any transport or permission
    /// failure and [`TrustError::Empty`] when the parameter carries no value.
    pub async fn read(&self, name: &str) -> Result<Zeroizing<String>, TrustError> {
        let output = self
            .client
            .get_parameter()
            .name(name)
            // Harmless on a `String` parameter and required for the
            // `SecureString` the cursor ring is held as. Asking for it
            // unconditionally means one code path rather than two.
            .with_decryption(true)
            .send()
            .await
            .map_err(|error| TrustError::Unreadable {
                name: name.to_owned(),
                reason: error.to_string(),
            })?;
        let value = output
            .parameter
            .and_then(|parameter| parameter.value)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| TrustError::Empty {
                name: name.to_owned(),
            })?;
        Ok(Zeroizing::new(value))
    }

    /// Reads and decodes the assertion trust anchors.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] for an unreadable or unusable document.
    pub async fn trust_anchors(&self, name: &str) -> Result<VerificationKeySet, TrustError> {
        let document = self.read(name).await?;
        parse_trust_anchors(name, &document)
    }

    /// Reads and decodes the cursor signing ring.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] for an unreadable or unusable document.
    pub async fn cursor_key_ring(&self, reference: &str) -> Result<CursorKeyRing, TrustError> {
        let document = self.read(reference).await?;
        parse_cursor_key_ring(reference, &document)
    }
}

// ---------------------------------------------------------------------------
// The `central-authz` assertion exchange
// ---------------------------------------------------------------------------

/// Builds the `central-authz` request for one presented workspace key.
///
/// The credential is named by `(keyId, presentedDigest)` and never sent: see
/// [`ResolveWorkspaceKey`] for why. The digest is `SHA-256` over the complete
/// token, which is exactly what the stored verifier is a MAC over, so naming the
/// key proves as much as sending it would.
///
/// # Errors
///
/// Returns [`AuthFailure::MalformedCredential`] for a key minted for another
/// region. That is refused here rather than centrally so the customer's secret
/// never leaves the region that received it, even as a digest.
pub fn resolve_request(
    credential: &PresentedCredential,
    audience: AssertionAudience,
    region: Region,
) -> Result<ResolveWorkspaceKey, AuthFailure> {
    if credential.region() != region {
        return Err(AuthFailure::MalformedCredential);
    }
    Ok(ResolveWorkspaceKey {
        schema_version: SchemaVersion::V1,
        key: credential.key_id(),
        presented_digest: CredentialDigest::new(*credential.digest().as_bytes()),
        region,
        audience,
    })
}

/// Converts a `central-authz` answer into the envelope the edge verifies.
///
/// There is nothing to cross-check here. The credential binding is inside the
/// signed body at a fixed offset, so an envelope minted for another credential
/// fails `verify` rather than needing a sibling field compared first — and,
/// because the cache stores only verified authorizations, a mismatched answer
/// can never reach it.
///
/// # Errors
///
/// Returns [`AuthFailure::Refused`] or [`AuthFailure::AccountStateUnavailable`]
/// for the authority's two decisions, and
/// [`AuthFailure::MalformedAssertion`] for an answer that is not a well-formed
/// 323-byte envelope.
pub fn issued_assertion(response: AssertionResponse) -> Result<Assertion, AuthFailure> {
    match response {
        AssertionResponse::Issued { assertion } => {
            Assertion::from_issued(&assertion).map_err(|_| AuthFailure::MalformedAssertion)
        }
        AssertionResponse::Refused {
            reason: AssertionRefusal::NotAuthorized,
        } => Err(AuthFailure::Refused),
        AssertionResponse::Refused {
            reason: AssertionRefusal::AccountStateUnavailable,
        } => Err(AuthFailure::AccountStateUnavailable),
    }
}

/// Decodes one `central-authz` response payload.
///
/// # Errors
///
/// Returns [`AuthFailure::MalformedAssertion`] for an oversized or undecodable
/// payload.
pub fn decode_response(payload: &[u8]) -> Result<AssertionResponse, AuthFailure> {
    if payload.len() > MAX_ASSERTION_RESPONSE_BYTES {
        return Err(AuthFailure::MalformedAssertion);
    }
    serde_json::from_slice(payload).map_err(|_| AuthFailure::MalformedAssertion)
}

/// The `central-authz` assertion exchange, over a direct Lambda invoke.
///
/// `central-authz` is IAM-invoked rather than routed, so there is no endpoint to
/// address and no second authentication hop: the caller's execution role *is*
/// the authentication, and a role without `lambda:InvokeFunction` on this one
/// function ARN cannot resolve an assertion at all.
#[derive(Debug, Clone)]
pub struct LambdaAssertionSource {
    client: aws_sdk_lambda::Client,
    function: String,
    audience: AssertionAudience,
    region: Region,
}

impl LambdaAssertionSource {
    /// Binds the exchange to one function, one audience and one region.
    ///
    /// The audience is fixed at construction because a deployable has exactly
    /// one: an assertion minted for the secret edge must never be accepted by
    /// the session edge, and asking for the right one is cheaper than checking
    /// afterwards.
    #[must_use]
    pub const fn new(
        client: aws_sdk_lambda::Client,
        function: String,
        audience: AssertionAudience,
        region: Region,
    ) -> Self {
        Self {
            client,
            function,
            audience,
            region,
        }
    }
}

#[async_trait]
impl AssertionSource for LambdaAssertionSource {
    async fn obtain(&self, credential: &PresentedCredential) -> Result<Assertion, AuthFailure> {
        let request = resolve_request(credential, self.audience, self.region)?;
        let body = serde_json::to_vec(&request).map_err(|_| AuthFailure::MalformedCredential)?;
        let output = self
            .client
            .invoke()
            .function_name(&self.function)
            .invocation_type(aws_sdk_lambda::types::InvocationType::RequestResponse)
            .payload(aws_sdk_lambda::primitives::Blob::new(body))
            .send()
            .await
            .map_err(|_| AuthFailure::SourceUnavailable)?;
        // A handled function error carries a payload that is *not* an answer.
        // Treating it as unavailable rather than trying to decode it keeps a
        // central failure from being reported to the customer as a bad key.
        if output.function_error.is_some() {
            return Err(AuthFailure::SourceUnavailable);
        }
        let payload = output
            .payload
            .as_ref()
            .map(aws_sdk_lambda::primitives::Blob::as_ref)
            .ok_or(AuthFailure::SourceUnavailable)?;
        issued_assertion(decode_response(payload)?)
    }
}

// ---------------------------------------------------------------------------
// The regional authorization projection
// ---------------------------------------------------------------------------

/// The regional authorization projection, over the read-only `DynamoDB` reader.
///
/// The table is written only by `central-control-worker`; every regional role
/// holds `GetItem` and `Query` on it and nothing else.
#[derive(Debug, Clone)]
pub struct RegionalProjection<P> {
    projection: P,
    region: Region,
}

impl<P> RegionalProjection<P> {
    /// Binds the projection to the region this process is pinned to.
    #[must_use]
    pub const fn new(projection: P, region: Region) -> Self {
        Self { projection, region }
    }
}

#[async_trait]
impl<P: AuthorizationProjection> ProjectionReader for RegionalProjection<P> {
    async fn project(
        &self,
        credential: &PresentedCredential,
    ) -> Result<ProjectedEpochs, ProjectionError> {
        if credential.region() != self.region {
            return Err(ProjectionError::Unknown);
        }
        // The projection holds a revocation row only for a key that has one, so
        // an absent row is the zero floor rather than a missing answer. A row
        // that exists raises the key floor above every assertion minted before
        // the revocation was published, which is what makes a revoked key lose
        // inside the 30-second assertion window.
        match self
            .projection
            .read_key_revocation(credential.key_id())
            .await
        {
            Ok(None) => Ok(ProjectedEpochs::default()),
            Ok(Some(revocation)) => Ok(ProjectedEpochs {
                key: Epoch::new(revocation.revoked_epoch),
                ..ProjectedEpochs::default()
            }),
            Err(_) => Err(ProjectionError::Unavailable),
        }
    }

    async fn placement(&self, workspace: WorkspaceId) -> Result<ProjectedState, ProjectionError> {
        let placement = self
            .projection
            .read_placement(workspace)
            .await
            .map_err(|error| placement_failure(&error))?;
        let region = Region::from_name(&placement.region).ok_or(ProjectionError::Unavailable)?;
        Ok(ProjectedState {
            epochs: ProjectedEpochs {
                key: Epoch::new(placement.key_epoch),
                workspace: Epoch::new(placement.revocation_epoch),
                account: Epoch::new(placement.account_epoch),
            },
            organization_id: raw_id(placement.organization),
            account_state: account_state(&placement.status)?,
            region,
        })
    }
}

/// The raw payload of a prefixed identifier, as the envelope binds it.
fn raw_id<I: aex_wire::ids::PrefixedId>(id: I) -> Uuid {
    Uuid::from_bytes(*id.uuid7().as_bytes())
}

/// An absent placement is "this region has no record of that workspace"; every
/// other store failure is "this region could not answer". The two are different
/// to a customer: the first is a `401`, the second a `503`.
fn placement_failure(error: &StoreError) -> ProjectionError {
    match error {
        StoreError::Misconfigured { .. } => ProjectionError::Unknown,
        _ => ProjectionError::Unavailable,
    }
}

/// Projects the stored placement status onto the account policy the edge gates
/// on.
///
/// `deleting` maps to `paused` rather than to a refusal: the pause-exempt set is
/// exactly the routes a deleting workspace still needs — trash and purge — and
/// every route that would start new paid work is outside it. A status outside
/// the closed vocabulary is a corrupt row, not a state to guess at.
fn account_state(status: &str) -> Result<AccountState, ProjectionError> {
    match status {
        "active" => Ok(AccountState::Active),
        "paused" | "deleting" => Ok(AccountState::Paused),
        _ => Err(ProjectionError::Unavailable),
    }
}
