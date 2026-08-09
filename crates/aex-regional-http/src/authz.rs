//! The concrete peer adapters a regional composition root needs to build an
//! edge: the credential pepper ring every presented key is checked against, the
//! cursor signing ring every continuation is minted under, and the regional
//! authorization projection.
//!
//! [`crate::edge::RegionalEdge`] deliberately keeps the projection as a port.
//! This module is where the ports stop being ports, and it is the only place in
//! the regional plane that names an AWS client for an authorization concern.
//!
//! # What is read when
//!
//! | Input | When | How often |
//! | --- | --- | --- |
//! | credential pepper ring ([`ParameterStore::pepper_ring`]) | cold start | once |
//! | cursor signing ring ([`ParameterStore::cursor_key_ring`]) | cold start | once |
//! | the admission snapshot ([`RegionalProjection`]) | per request | always |
//!
//! The snapshot is the only per-request read — three concurrent point reads over
//! the key authorization row, the placement and the hot limit ceilings,
//! reconciled after decode — and it is deliberately never cached. It is now the
//! *whole* authorization answer rather than a check on a cached one: there is no
//! central assertion, no 30-second lifetime and nothing held between requests,
//! so a revoked key or a paused account takes effect on the next request rather
//! than within a window.
//!
//! # There is no `central-authz` client here
//!
//! There used to be: a direct `lambda:Invoke` per credential per thirty
//! seconds. The verifier the authority compared against is now replicated onto
//! the key authorization row this module already reads, and the comparison
//! happens in [`crate::credential::PepperRing::admits`]. Nothing regional
//! reaches the central plane on a request path.

use aex_control_domain::epoch::Epoch;
use aex_identity_domain::credential::Pepper;
use aex_internal_contracts::SchemaVersion;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::projection::AuthorizationProjection;
use aex_session_dynamodb::wire_pending::{EdgeLimits, KeyAuthorizationState};
use aex_wire::ids::{ApiKeyId, WorkspaceId};
use aex_wire::types::Region;
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use std::collections::BTreeMap;
use uuid::Uuid;
use zeroize::{Zeroize as _, Zeroizing};

use crate::context::{AccountState, EffectiveLimits};
use crate::credential::{PepperRing, ProjectedEpochs, RingError, StoredVerifier};
use crate::cursor::{CursorError, CursorKey, CursorKeyRing};
use crate::edge::{ProjectedState, ProjectionError, ProjectionReader};

/// The largest parameter document this module will decode.
pub const MAX_PARAMETER_BYTES: usize = 64 * 1_024;

/// Why start-up key material was refused.
///
/// Every variant stops the process. There is no arm that degrades to an empty
/// ring or a generated pepper: a regional edge that cannot check a credential
/// must not start, because the alternative is a process that accepts nothing
/// while reporting ready, or worse, one that accepts everything.
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
    /// Two cursor keys claimed the same identity.
    #[error("parameter `{name}` declares `{key_id}` twice")]
    DuplicateKeyId {
        /// Which parameter.
        name: String,
        /// The repeated identity.
        key_id: String,
    },
    /// Two peppers claimed the same version.
    #[error("parameter `{name}` declares pepper version {version} twice")]
    DuplicatePepperVersion {
        /// Which parameter.
        name: String,
        /// The repeated version.
        version: u16,
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
    /// The pepper ring itself refused the entries.
    #[error("parameter `{name}` does not describe a usable pepper ring: {source}")]
    PepperRing {
        /// Which parameter.
        name: String,
        /// Why the ring refused them.
        source: RingError,
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
// The credential pepper ring
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PepperDocument {
    #[allow(
        dead_code,
        reason = "decoded so an unversioned or future document is refused rather than guessed at"
    )]
    schema_version: SchemaVersion,
    peppers: Vec<PepperEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PepperEntry {
    version: u16,
    material: String,
}

/// Decodes the credential pepper ring held at `AEX_CREDENTIAL_PEPPER_REF`.
///
/// The reference is a Parameter Store name holding a `SecureString`. The
/// document is byte-for-byte the one the issuing authority reads, because it is
/// the same ring: a second spelling would be a second thing that can disagree
/// about which pepper version `3` is.
///
/// ```json
/// { "schemaVersion": 1,
///   "peppers": [ { "version": 1, "material": "<43 chars unpadded base64url>" } ] }
/// ```
///
/// Every version the region may still see has to be present. A verifier names
/// the version it was computed under, so a ring missing an older entry does not
/// refuse those credentials — it reports that it cannot check them, which is a
/// `503` an operator will notice rather than a `401` a customer will.
///
/// # Errors
///
/// Returns [`TrustError`] for an oversized, malformed, empty, over-long or
/// ambiguous document, or one declaring material that is not exactly 32 bytes.
/// Nothing here degrades: a document this function refuses stops the process.
pub fn parse_pepper_ring(name: &str, document: &str) -> Result<PepperRing, TrustError> {
    if document.len() > MAX_PARAMETER_BYTES {
        return Err(TrustError::TooLarge {
            name: name.to_owned(),
            found: document.len(),
        });
    }
    let mut decoded: PepperDocument =
        serde_json::from_str(document).map_err(|error| TrustError::Malformed {
            name: name.to_owned(),
            kind: "credential pepper ring",
            reason: error.to_string(),
        })?;
    let mut peppers = BTreeMap::new();
    for entry in &mut decoded.peppers {
        let material = decode_32(&mut entry.material).ok_or_else(|| TrustError::KeyMaterial {
            name: name.to_owned(),
            key_id: entry.version.to_string(),
        })?;
        if peppers
            .insert(entry.version, Pepper::new(*material))
            .is_some()
        {
            return Err(TrustError::DuplicatePepperVersion {
                name: name.to_owned(),
                version: entry.version,
            });
        }
    }
    PepperRing::new(peppers).map_err(|source| TrustError::PepperRing {
        name: name.to_owned(),
        source,
    })
}

/// Decodes exactly 32 secret bytes, zeroizing the encoded material it consumed.
fn decode_32(encoded: &mut String) -> Option<Zeroizing<[u8; 32]>> {
    let decoded = aex_control_domain::codec::unbase64url(encoded);
    encoded.zeroize();
    let mut bytes = Zeroizing::new(decoded?);
    let material = <[u8; 32]>::try_from(bytes.as_slice()).ok();
    bytes.zeroize();
    material.map(Zeroizing::new)
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
            // Required for the `SecureString` both rings are held as. Asking for
            // it unconditionally means one code path rather than two.
            .with_decryption(true)
            .send()
            .await
            .map_err(|error| TrustError::Unreadable {
                name: name.to_owned(),
                // `SdkError`'s own `Display` is the bare word "service error"
                // for every service failure, so an access denial, a throttle and
                // an absent parameter all read identically in the last line this
                // process writes before it exits. `DisplayErrorContext` walks the
                // source chain and names the code, which is the difference
                // between "fix the role" and "fix the parameter".
                reason: aws_sdk_ssm::error::DisplayErrorContext(&error).to_string(),
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

    /// Reads and decodes the credential pepper ring.
    ///
    /// # Errors
    ///
    /// Returns [`TrustError`] for an unreadable or unusable document.
    pub async fn pepper_ring(&self, reference: &str) -> Result<PepperRing, TrustError> {
        let document = self.read(reference).await?;
        parse_pepper_ring(reference, &document)
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
// The regional authorization projection
// ---------------------------------------------------------------------------

/// The regional authorization projection, over the read-only `DynamoDB` reader.
///
/// Key authorization, placement and profile rows are written only by
/// `central-control-worker` and limit rows only by the regional capacity
/// authority; every serving regional role holds read actions on the table and
/// nothing else.
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
    async fn snapshot(
        &self,
        key: ApiKeyId,
        workspace: WorkspaceId,
    ) -> Result<ProjectedState, ProjectionError> {
        let snapshot = self
            .projection
            .read_admission_snapshot(key, workspace)
            .await
            .map_err(|error| snapshot_failure(&error))?;
        let placement = &snapshot.placement;
        // The key row's own region and the placement's must agree, and both must
        // be this endpoint's. Checking here rather than only at the edge means a
        // key projected into the wrong region is refused by the reader that
        // produced it.
        let region = Region::from_name(&placement.region).ok_or(ProjectionError::Unavailable)?;
        if snapshot.key.region != placement.region || region != self.region {
            return Err(ProjectionError::Unknown);
        }
        Ok(ProjectedState {
            // The key row carries the floor for the key itself; the placement
            // carries the two workspace-scoped floors. Taking the key floor from
            // the row that is raised by revocation is what lets a long-lived
            // lease notice a revocation it was opened before.
            epochs: ProjectedEpochs {
                key: Epoch::new(placement.key_epoch.max(snapshot.key.key_epoch)),
                workspace: Epoch::new(placement.revocation_epoch),
                account: Epoch::new(placement.account_epoch),
            },
            workspace_id: snapshot.key.workspace,
            organization_id: raw_id(placement.organization),
            key_revoked: snapshot.key.state == KeyAuthorizationState::Revoked,
            verifier: StoredVerifier::new(snapshot.key.verifier, snapshot.key.pepper_version),
            audiences: snapshot.key.audiences,
            scopes: snapshot.key.scopes,
            account_state: account_state(&placement.status)?,
            region,
            limits: effective(&snapshot.limits)?,
        })
    }
}

/// The raw payload of a prefixed identifier.
fn raw_id<I: aex_wire::ids::PrefixedId>(id: I) -> Uuid {
    Uuid::from_bytes(*id.uuid7().as_bytes())
}

/// An absent or self-contradictory snapshot is "this region has no usable record
/// of that key"; every other store failure is "this region could not answer".
/// The two are different to a customer: the first is a `401`, the second a
/// `503`.
fn snapshot_failure(error: &StoreError) -> ProjectionError {
    match error {
        StoreError::Misconfigured { .. } | StoreError::Corrupt(_) => ProjectionError::Unknown,
        _ => ProjectionError::Unavailable,
    }
}

/// Narrows the projected ceilings onto the machine-word type the edge applies.
///
/// A ceiling that does not fit a `usize` cannot be enforced on this host, and an
/// unenforceable ceiling is refused rather than saturated: saturating it would
/// silently admit a body the authority never permitted.
fn effective(limits: &EdgeLimits) -> Result<EffectiveLimits, ProjectionError> {
    let width = |value: u64| usize::try_from(value).map_err(|_| ProjectionError::Unavailable);
    Ok(EffectiveLimits {
        json_body_bytes: width(limits.json_body_bytes)?,
        otlp_body_bytes: width(limits.otlp_body_bytes)?,
        query_page_items: width(limits.query_page_items)?,
        query_page_bytes: width(limits.query_page_bytes)?,
    })
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
