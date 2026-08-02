//! The one signed regional cursor codec.

use std::collections::BTreeSet;
use std::fmt;

use aex_wire::cursor::Cursor;
use aex_wire::ids::{SessionId, WorkspaceId};
use aex_wire::routes::RouteId;
use aex_wire::types::{Region, Timestamp};
use base64::Engine as _;
use hmac::{KeyInit as _, Mac as _};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The hard `cursor.snapshot` lifetime.
pub const SNAPSHOT_MILLIS: i64 = 24 * 60 * 60 * 1_000;
/// Minimum HMAC key length.
pub const MIN_KEY_BYTES: usize = 32;
const TAG_BYTES: usize = 32;
const MAX_TUPLE_PARTS: usize = 8;
const MAX_TUPLE_PART_BYTES: usize = 512;

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// Stable sort direction bound into a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Smallest tuple first.
    Ascending,
    /// Largest tuple first.
    Descending,
}

/// The authority snapshot a continuation belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotToken(String);

impl SnapshotToken {
    /// Validates a non-secret opaque snapshot token.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::Malformed`] outside the closed bound.
    pub fn new(value: impl Into<String>) -> Result<Self, CursorError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 256
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(CursorError::Malformed);
        }
        Ok(Self(value))
    }

    /// Borrow the authenticated snapshot spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every fact that prevents a cursor from being replayed in another context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBinding {
    /// Generated route identity.
    pub route: RouteId,
    /// Digest of the effective principal scope.
    pub principal_scope: [u8; 32],
    /// Immutable workspace region.
    pub region: Region,
    /// Workspace authority.
    pub workspace_id: WorkspaceId,
    /// Optional session authority.
    pub session_id: Option<SessionId>,
    /// Digest of the normalized query.
    pub query_hash: [u8; 32],
    /// Stable sort direction.
    pub order: Order,
    /// Captured authority snapshot.
    pub snapshot: SnapshotToken,
}

/// Stable request facts required to resume a cursor whose snapshot is carried
/// inside the authenticated token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorRequestBinding {
    /// Generated route identity (or a canonical route shared by a stream/listen pair).
    pub route: RouteId,
    /// Digest of the effective principal scope.
    pub principal_scope: [u8; 32],
    /// Immutable workspace region.
    pub region: Region,
    /// Workspace authority.
    pub workspace_id: WorkspaceId,
    /// Optional session authority.
    pub session_id: Option<SessionId>,
    /// Digest of the stable request filter and signal selection.
    pub query_hash: [u8; 32],
    /// Stable sort direction.
    pub order: Order,
}

impl From<&CursorBinding> for CursorRequestBinding {
    fn from(binding: &CursorBinding) -> Self {
        Self {
            route: binding.route,
            principal_scope: binding.principal_scope,
            region: binding.region,
            workspace_id: binding.workspace_id,
            session_id: binding.session_id,
            query_hash: binding.query_hash,
            order: binding.order,
        }
    }
}

/// Authenticated resume state recovered from a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorResume {
    /// The original settled snapshot.
    pub snapshot: SnapshotToken,
    /// The last fully delivered ordering tuple.
    pub tuple: SortTuple,
}

/// The ordered authority fields needed to resume a read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SortTuple(Vec<String>);

impl SortTuple {
    /// Builds a bounded tuple containing no control bytes.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::Malformed`] for an empty or oversized tuple.
    pub fn new(parts: Vec<String>) -> Result<Self, CursorError> {
        if parts.is_empty()
            || parts.len() > MAX_TUPLE_PARTS
            || parts.iter().any(|part| {
                part.is_empty()
                    || part.len() > MAX_TUPLE_PART_BYTES
                    || part.bytes().any(|byte| byte.is_ascii_control())
            })
        {
            return Err(CursorError::Malformed);
        }
        Ok(Self(parts))
    }

    /// Tuple fields in authority order.
    #[must_use]
    pub fn parts(&self) -> &[String] {
        &self.0
    }
}

/// One cursor signing key.
pub struct CursorKey {
    id: String,
    material: Zeroizing<Vec<u8>>,
}

impl CursorKey {
    /// Validates key identity and minimum material length.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::WeakKey`] or [`CursorError::Malformed`].
    pub fn new(id: impl Into<String>, material: Vec<u8>) -> Result<Self, CursorError> {
        let id = id.into();
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(CursorError::Malformed);
        }
        if material.len() < MIN_KEY_BYTES {
            return Err(CursorError::WeakKey {
                found: material.len(),
            });
        }
        Ok(Self {
            id,
            material: Zeroizing::new(material),
        })
    }
}

impl fmt::Debug for CursorKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CursorKey")
            .field("id", &self.id)
            .field("material", &"<redacted>")
            .finish()
    }
}

/// Current key plus explicitly bounded rotation-overlap keys.
#[derive(Debug)]
pub struct CursorKeyRing {
    keys: Vec<CursorKey>,
}

impl CursorKeyRing {
    /// Constructs a ring, refusing duplicate ids or more than two overlap keys.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::Malformed`] for an ambiguous ring.
    pub fn new(current: CursorKey, overlap: Vec<CursorKey>) -> Result<Self, CursorError> {
        if overlap.len() > 2 {
            return Err(CursorError::Malformed);
        }
        let mut keys = Vec::with_capacity(overlap.len() + 1);
        keys.push(current);
        keys.extend(overlap);
        let ids = keys
            .iter()
            .map(|key| key.id.as_str())
            .collect::<BTreeSet<_>>();
        if ids.len() != keys.len() {
            return Err(CursorError::Malformed);
        }
        Ok(Self { keys })
    }

    /// The key every newly minted cursor is signed under.
    ///
    /// Rotation-overlap keys verify but never sign, so a ring cannot keep
    /// issuing under a key that is on its way out.
    ///
    /// # Panics
    ///
    /// Never: [`CursorKeyRing::new`] takes the current key by value, so the ring
    /// cannot be constructed empty.
    #[must_use]
    pub fn current(&self) -> &CursorKey {
        self.keys
            .first()
            .expect("a ring always holds its current key")
    }
}

/// Why a cursor could not be issued or consumed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    /// Key material was shorter than the security floor.
    #[error("cursor key has {found} bytes; at least {MIN_KEY_BYTES} are required")]
    WeakKey {
        /// Observed key length.
        found: usize,
    },
    /// The token was not a valid cursor envelope.
    #[error("malformed cursor")]
    Malformed,
    /// The signature or full binding did not match.
    #[error("cursor is not valid for this request")]
    NotBound,
    /// The token exceeded the fixed snapshot lifetime.
    #[error("cursor expired")]
    Expired,
}

#[derive(Serialize, Deserialize)]
struct Payload {
    v: u8,
    kid: String,
    route: String,
    principal: [u8; 32],
    region: Region,
    workspace: String,
    session: Option<String>,
    query: [u8; 32],
    order: Order,
    snapshot: SnapshotToken,
    issued_at_ms: i64,
    tuple: SortTuple,
}

/// Encodes an authenticated `cur_` token using the workspace JCS implementation.
///
/// # Errors
///
/// Returns [`CursorError::Malformed`] if the bounded payload cannot be encoded.
pub fn encode(
    key: &CursorKey,
    binding: &CursorBinding,
    tuple: &SortTuple,
    now: Timestamp,
) -> Result<Cursor, CursorError> {
    let payload = Payload {
        v: 1,
        kid: key.id.clone(),
        route: binding.route.as_str().to_owned(),
        principal: binding.principal_scope,
        region: binding.region,
        workspace: binding.workspace_id.to_string(),
        session: binding.session_id.map(|id| id.to_string()),
        query: binding.query_hash,
        order: binding.order,
        snapshot: binding.snapshot.clone(),
        issued_at_ms: now.unix_millis(),
        tuple: tuple.clone(),
    };
    let body = aex_wire::canonical::to_jcs_bytes(&payload).map_err(|_| CursorError::Malformed)?;
    let tag = sign(key, &body);
    let mut envelope = Vec::with_capacity(TAG_BYTES + body.len());
    envelope.extend_from_slice(&tag);
    envelope.extend_from_slice(&body);
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope);
    Cursor::parse(&format!("{}{encoded}", Cursor::PREFIX)).map_err(|_| CursorError::Malformed)
}

/// Verifies the token before parsing its authenticated payload.
///
/// # Errors
///
/// Returns [`CursorError`] for malformed, unbound or expired tokens.
pub fn decode(
    ring: &CursorKeyRing,
    token: &Cursor,
    binding: &CursorBinding,
    now: Timestamp,
) -> Result<SortTuple, CursorError> {
    let resumed = decode_resume(ring, token, &CursorRequestBinding::from(binding), now)?;
    if resumed.snapshot != binding.snapshot {
        return Err(CursorError::NotBound);
    }
    Ok(resumed.tuple)
}

/// Verifies a cursor and returns its authenticated snapshot and tuple.
///
/// This is used by long-lived stream reconnects: the reconnect request carries
/// a cursor rather than restating the old snapshot. Every other request fact is
/// still compared before the snapshot is exposed.
///
/// # Errors
///
/// Returns [`CursorError`] for malformed, unbound or expired tokens.
pub fn decode_resume(
    ring: &CursorKeyRing,
    token: &Cursor,
    binding: &CursorRequestBinding,
    now: Timestamp,
) -> Result<CursorResume, CursorError> {
    let encoded = token
        .as_str()
        .strip_prefix(Cursor::PREFIX)
        .ok_or(CursorError::Malformed)?;
    let envelope = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| CursorError::Malformed)?;
    if envelope.len() <= TAG_BYTES {
        return Err(CursorError::Malformed);
    }
    let (tag, body) = envelope.split_at(TAG_BYTES);
    let matched = ring
        .keys
        .iter()
        .find(|key| constant_time_eq(tag, &sign(key, body)))
        .ok_or(CursorError::NotBound)?;
    let payload: Payload = serde_json::from_slice(body).map_err(|_| CursorError::Malformed)?;
    if payload.v != 1
        || payload.kid != matched.id
        || payload.route != binding.route.as_str()
        || payload.principal != binding.principal_scope
        || payload.region != binding.region
        || payload.workspace != binding.workspace_id.to_string()
        || payload.session != binding.session_id.map(|id| id.to_string())
        || payload.query != binding.query_hash
        || payload.order != binding.order
    {
        return Err(CursorError::NotBound);
    }
    let age = now.unix_millis().saturating_sub(payload.issued_at_ms);
    if age < 0 {
        return Err(CursorError::NotBound);
    }
    if age > SNAPSHOT_MILLIS {
        return Err(CursorError::Expired);
    }
    Ok(CursorResume {
        snapshot: payload.snapshot,
        tuple: payload.tuple,
    })
}

fn sign(key: &CursorKey, body: &[u8]) -> [u8; TAG_BYTES] {
    let mut mac = HmacSha256::new_from_slice(&key.material).expect("HMAC accepts any key length");
    mac.update(body);
    mac.finalize().into_bytes().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
