//! The opaque signed keyset cursor.
//!
//! A cursor is a `cur_`-prefixed base64url payload plus a constant-time-checked
//! HMAC. Every scope field the query was bound by is inside the signed payload,
//! so a cursor minted for one principal, endpoint, scope, region or filter
//! cannot be replayed against another — it fails the comparison rather than
//! silently paging over rows the caller may not see.
//!
//! Cursors expire after 24 hours. A keyset cursor is a position in an ordering,
//! and an ordering that has since been re-paged is not a position any more.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::codec::{base64url, unbase64url};

/// How long a cursor stays usable.
pub const CURSOR_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// The wire prefix.
pub const CURSOR_PREFIX: &str = "cur_";

/// Maximum accepted wire length, matching the public cursor limit.
pub const CURSOR_MAX_ENCODED_BYTES: usize = 4_096;

const MAGIC: &[u8; 4] = b"AEXC";
const VERSION: u8 = 2;

/// The keyed secret a cursor is signed with.
///
/// Same shape and rotation as a credential pepper, different purpose, so a
/// cursor MAC can never be confused with a credential verifier.
#[derive(Clone)]
pub struct CursorSecret {
    current: CursorKey,
    overlap: Vec<CursorKey>,
}

#[derive(Clone)]
struct CursorKey {
    id: u16,
    material: Zeroizing<[u8; 32]>,
}

impl CursorSecret {
    /// Wraps 32 secret bytes.
    #[must_use]
    pub fn new(bytes: [u8; 32]) -> Self {
        Self::with_id(1, bytes)
    }

    /// Wraps the active key version and its 32 secret bytes.
    #[must_use]
    pub fn with_id(id: u16, bytes: [u8; 32]) -> Self {
        Self {
            current: CursorKey {
                id,
                material: Zeroizing::new(bytes),
            },
            overlap: Vec::new(),
        }
    }

    /// Adds up to two unique verification-only keys for a future rolling rotation.
    ///
    /// # Errors
    ///
    /// Returns [`CursorKeyRingError`] for a duplicate key ID or more than two
    /// overlap keys.
    pub fn with_overlap(
        mut self,
        overlap: impl IntoIterator<Item = (u16, [u8; 32])>,
    ) -> Result<Self, CursorKeyRingError> {
        for (id, bytes) in overlap {
            if self.overlap.len() == 2 {
                return Err(CursorKeyRingError::TooManyKeys);
            }
            if id == self.current.id || self.overlap.iter().any(|key| key.id == id) {
                return Err(CursorKeyRingError::DuplicateKeyId);
            }
            self.overlap.push(CursorKey {
                id,
                material: Zeroizing::new(bytes),
            });
        }
        Ok(self)
    }

    fn verification_key(&self, id: u16) -> Option<&CursorKey> {
        if self.current.id == id {
            Some(&self.current)
        } else {
            self.overlap.iter().find(|key| key.id == id)
        }
    }
}

/// Invalid cursor verification-key configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorKeyRingError {
    /// Current and overlap key IDs must be unique.
    #[error("cursor key IDs must be unique")]
    DuplicateKeyId,
    /// One current and at most two overlap keys are supported.
    #[error("a cursor key ring permits at most two overlap keys")]
    TooManyKeys,
}

impl std::fmt::Debug for CursorSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted:32 bytes>")
    }
}

/// Everything a cursor is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorClaims {
    /// The canonical route template the page belongs to.
    pub endpoint: String,
    /// Which principal minted it.
    pub principal_id: Uuid,
    /// Which organization or workspace the query was scoped to.
    pub scope_id: Uuid,
    /// The region the query ran in.
    pub region: aex_wire::types::Region,
    /// A digest over every filter the query applied.
    pub filter_hash: [u8; 32],
    /// When the page was first taken.
    pub snapshot_ms: i64,
    /// The last `(created_at_ms, id)` returned.
    pub last: (i64, Uuid),
}

/// Why a cursor was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    /// The value did not have the `cur_<payload>.<mac>` shape.
    #[error("the cursor is not a well-formed AEX cursor")]
    Malformed,
    /// The MAC did not verify.
    #[error("the cursor did not verify")]
    BadSignature,
    /// The cursor was minted more than [`CURSOR_TTL_MS`] ago.
    #[error("the cursor expired")]
    Expired,
    /// The cursor was minted for a different query.
    #[error("the cursor belongs to a different query")]
    ScopeMismatch,
}

/// The domain-separating prefix.
const DOMAIN: &[u8] = b"aex/control/cursor/v2\x1f";

/// The signed payload, as fixed-layout bytes.
///
/// A fixed layout means the payload has no delimiters to confuse, so two
/// different claim sets can never render to the same bytes.
///
/// # Panics
///
/// Never: the endpoint is a generated route template, far shorter than 4 GiB.
fn payload(claims: &CursorClaims, issued_at_ms: i64, key_id: u16) -> Vec<u8> {
    let endpoint = claims.endpoint.as_bytes();
    let mut bytes = Vec::with_capacity(128 + endpoint.len());
    bytes.extend_from_slice(MAGIC);
    bytes.push(VERSION);
    bytes.extend_from_slice(&key_id.to_be_bytes());
    bytes.extend_from_slice(&issued_at_ms.to_be_bytes());
    bytes.extend_from_slice(claims.principal_id.as_bytes());
    bytes.extend_from_slice(claims.scope_id.as_bytes());
    bytes.push(region_code(claims.region));
    bytes.extend_from_slice(&claims.filter_hash);
    bytes.extend_from_slice(&claims.snapshot_ms.to_be_bytes());
    bytes.extend_from_slice(&claims.last.0.to_be_bytes());
    bytes.extend_from_slice(claims.last.1.as_bytes());
    let endpoint_len =
        u32::try_from(endpoint.len()).expect("a route template is far shorter than 4 GiB");
    bytes.extend_from_slice(&endpoint_len.to_be_bytes());
    bytes.extend_from_slice(endpoint);
    bytes
}

/// The one-byte region discriminant, in wire-contract order.
#[must_use]
pub const fn region_code(region: aex_wire::types::Region) -> u8 {
    match region {
        aex_wire::types::Region::UsEast1 => 1,
        aex_wire::types::Region::UsEast2 => 2,
        aex_wire::types::Region::UsWest2 => 3,
        aex_wire::types::Region::ApNortheast1 => 4,
        aex_wire::types::Region::EuWest1 => 5,
    }
}

/// Resolves a one-byte region discriminant.
#[must_use]
pub const fn region_from_code(code: u8) -> Option<aex_wire::types::Region> {
    match code {
        1 => Some(aex_wire::types::Region::UsEast1),
        2 => Some(aex_wire::types::Region::UsEast2),
        3 => Some(aex_wire::types::Region::UsWest2),
        4 => Some(aex_wire::types::Region::ApNortheast1),
        5 => Some(aex_wire::types::Region::EuWest1),
        _ => None,
    }
}

/// The MAC over one payload.
fn mac(key: &CursorKey, bytes: &[u8]) -> [u8; 32] {
    let mut hmac = <Hmac<Sha256> as KeyInit>::new_from_slice(key.material.as_ref())
        .expect("HMAC-SHA256 accepts a 32-byte key");
    hmac.update(DOMAIN);
    hmac.update(bytes);
    hmac.finalize().into_bytes().into()
}

/// Encodes a cursor.
#[must_use]
pub fn encode_cursor(secret: &CursorSecret, claims: &CursorClaims, now_ms: i64) -> String {
    let bytes = payload(claims, now_ms, secret.current.id);
    let tag = mac(&secret.current, &bytes);
    let encoded = format!("{CURSOR_PREFIX}{}.{}", base64url(&bytes), base64url(&tag));
    debug_assert!(encoded.len() <= CURSOR_MAX_ENCODED_BYTES);
    encoded
}

/// Decodes a cursor and checks every binding.
///
/// A *binding* is a field the query fixes and the caller therefore already
/// knows: the endpoint, the principal, the scope, the region and the filter
/// digest. `snapshot_ms` and `last` are **carried state** — they are what a
/// cursor exists to transport — so they are returned rather than compared.
/// Comparing them would make this function unusable for its only purpose, since
/// a caller that already knew the position would not need the cursor.
///
/// # Errors
///
/// Returns [`CursorError::Malformed`] for a shape failure,
/// [`CursorError::BadSignature`] for a MAC that does not verify,
/// [`CursorError::Expired`] past [`CURSOR_TTL_MS`], and
/// [`CursorError::ScopeMismatch`] when any bound field differs from `expected`.
///
/// The MAC is checked **before** the bindings, so a caller with a forged cursor
/// learns nothing about what the real bindings would have been.
pub fn decode_cursor(
    secret: &CursorSecret,
    raw: &str,
    expected: &CursorClaims,
    now_ms: i64,
) -> Result<(i64, Uuid), CursorError> {
    if raw.len() > CURSOR_MAX_ENCODED_BYTES {
        return Err(CursorError::Malformed);
    }
    let body = raw
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(CursorError::Malformed)?;
    let (payload_text, mac_text) = body.split_once('.').ok_or(CursorError::Malformed)?;
    let bytes = unbase64url(payload_text).ok_or(CursorError::Malformed)?;
    let presented = unbase64url(mac_text).ok_or(CursorError::Malformed)?;
    if presented.len() != 32 {
        return Err(CursorError::Malformed);
    }

    let key_id = parse_header(&bytes).ok_or(CursorError::Malformed)?;
    let key = secret
        .verification_key(key_id)
        .ok_or(CursorError::BadSignature)?;
    let expected_mac = mac(key, &bytes);
    if expected_mac.ct_eq(&presented).unwrap_u8() != 1 {
        return Err(CursorError::BadSignature);
    }

    let decoded = parse_payload(&bytes).ok_or(CursorError::Malformed)?;
    if decoded.issued_at_ms > now_ms || now_ms - decoded.issued_at_ms > CURSOR_TTL_MS {
        return Err(CursorError::Expired);
    }
    if !bindings_match(&decoded.claims, expected) {
        return Err(CursorError::ScopeMismatch);
    }
    Ok(decoded.claims.last)
}

/// Whether two claim sets agree on every field the query fixes.
fn bindings_match(decoded: &CursorClaims, expected: &CursorClaims) -> bool {
    decoded.endpoint == expected.endpoint
        && decoded.principal_id == expected.principal_id
        && decoded.scope_id == expected.scope_id
        && decoded.region == expected.region
        && decoded.filter_hash == expected.filter_hash
}

/// A decoded payload.
struct Decoded {
    issued_at_ms: i64,
    claims: CursorClaims,
}

fn parse_header(bytes: &[u8]) -> Option<u16> {
    if bytes.get(0..4)? != MAGIC || *bytes.get(4)? != VERSION {
        return None;
    }
    Some(u16::from_be_bytes(bytes.get(5..7)?.try_into().ok()?))
}

/// Parses the fixed-layout payload.
#[allow(
    clippy::missing_panics_doc,
    reason = "every slice index is bounds-checked before use"
)]
fn parse_payload(bytes: &[u8]) -> Option<Decoded> {
    const HEADER: usize = 4 + 1 + 2;
    const FIXED: usize = HEADER + 8 + 16 + 16 + 1 + 32 + 8 + 8 + 16 + 4;
    if bytes.len() < FIXED {
        return None;
    }
    parse_header(bytes)?;
    let issued_at_ms = i64::from_be_bytes(bytes[7..15].try_into().ok()?);
    let principal_id = Uuid::from_slice(&bytes[15..31]).ok()?;
    let scope_id = Uuid::from_slice(&bytes[31..47]).ok()?;
    let region = region_from_code(bytes[47])?;
    let filter_hash: [u8; 32] = bytes[48..80].try_into().ok()?;
    let snapshot_ms = i64::from_be_bytes(bytes[80..88].try_into().ok()?);
    let last_created = i64::from_be_bytes(bytes[88..96].try_into().ok()?);
    let last_id = Uuid::from_slice(&bytes[96..112]).ok()?;
    let endpoint_len = u32::from_be_bytes(bytes[112..116].try_into().ok()?) as usize;
    if bytes.len() != FIXED + endpoint_len {
        return None;
    }
    let endpoint = std::str::from_utf8(&bytes[116..116 + endpoint_len])
        .ok()?
        .to_owned();
    Some(Decoded {
        issued_at_ms,
        claims: CursorClaims {
            endpoint,
            principal_id,
            scope_id,
            region,
            filter_hash,
            snapshot_ms,
            last: (last_created, last_id),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CURSOR_MAX_ENCODED_BYTES, CURSOR_TTL_MS, CursorClaims, CursorError, CursorSecret,
        decode_cursor, encode_cursor,
    };
    use aex_wire::types::Region;
    use uuid::Uuid;

    fn secret() -> CursorSecret {
        CursorSecret::new([7_u8; 32])
    }

    fn claims() -> CursorClaims {
        CursorClaims {
            endpoint: "/api/workspaces".to_owned(),
            principal_id: Uuid::from_u128(1),
            scope_id: Uuid::from_u128(2),
            region: Region::EuWest1,
            filter_hash: [3_u8; 32],
            snapshot_ms: 1_767_225_600_000,
            last: (1_767_225_600_500, Uuid::from_u128(4)),
        }
    }

    #[test]
    fn a_cursor_round_trips_inside_its_window() {
        let raw = encode_cursor(&secret(), &claims(), 1_767_225_600_000);
        assert!(raw.starts_with("cur_"));
        assert_eq!(
            decode_cursor(&secret(), &raw, &claims(), 1_767_225_600_001),
            Ok(claims().last)
        );
    }

    #[test]
    fn a_mutated_mac_fails_the_constant_time_check() {
        let raw = encode_cursor(&secret(), &claims(), 0);
        let mut mutated: Vec<char> = raw.chars().collect();
        let last = mutated.len() - 1;
        mutated[last] = if mutated[last] == 'A' { 'B' } else { 'A' };
        let mutated: String = mutated.into_iter().collect();
        assert_eq!(
            decode_cursor(&secret(), &mutated, &claims(), 0),
            Err(CursorError::BadSignature)
        );
    }

    #[test]
    fn a_cursor_signed_with_another_secret_never_verifies() {
        let raw = encode_cursor(&CursorSecret::new([9_u8; 32]), &claims(), 0);
        assert_eq!(
            decode_cursor(&secret(), &raw, &claims(), 0),
            Err(CursorError::BadSignature)
        );
    }

    #[test]
    fn every_bound_field_is_checked() {
        let raw = encode_cursor(&secret(), &claims(), 0);
        let mutations: [(&str, CursorClaims); 5] = [
            (
                "endpoint",
                CursorClaims {
                    endpoint: "/api/api-keys".to_owned(),
                    ..claims()
                },
            ),
            (
                "principal",
                CursorClaims {
                    principal_id: Uuid::from_u128(99),
                    ..claims()
                },
            ),
            (
                "scope",
                CursorClaims {
                    scope_id: Uuid::from_u128(99),
                    ..claims()
                },
            ),
            (
                "region",
                CursorClaims {
                    region: Region::UsEast1,
                    ..claims()
                },
            ),
            (
                "filter",
                CursorClaims {
                    filter_hash: [4_u8; 32],
                    ..claims()
                },
            ),
        ];
        for (field, expected) in mutations {
            assert_eq!(
                decode_cursor(&secret(), &raw, &expected, 0),
                Err(CursorError::ScopeMismatch),
                "{field}"
            );
        }
    }

    #[test]
    fn a_continuation_is_readable_without_knowing_the_position_it_carries() {
        // The whole point of a cursor is that the caller does not know where the
        // previous page ended. A decoder that compared `last` could only ever be
        // called by somebody who already had the answer.
        let raw = encode_cursor(&secret(), &claims(), 0);
        let unknown = CursorClaims {
            last: (0, Uuid::nil()),
            snapshot_ms: 0,
            ..claims()
        };
        assert_eq!(
            decode_cursor(&secret(), &raw, &unknown, 0),
            Ok(claims().last)
        );
    }

    #[test]
    fn a_cursor_expires_after_twenty_four_hours() {
        let raw = encode_cursor(&secret(), &claims(), 0);
        assert!(decode_cursor(&secret(), &raw, &claims(), CURSOR_TTL_MS).is_ok());
        assert_eq!(
            decode_cursor(&secret(), &raw, &claims(), CURSOR_TTL_MS + 1),
            Err(CursorError::Expired)
        );
    }

    #[test]
    fn a_future_issued_cursor_is_refused_without_saturating_its_age() {
        let raw = encode_cursor(&secret(), &claims(), 1);
        assert_eq!(
            decode_cursor(&secret(), &raw, &claims(), 0),
            Err(CursorError::Expired)
        );
    }

    #[test]
    fn a_retiring_key_verifies_but_never_writes() {
        let old = CursorSecret::with_id(7, [7_u8; 32]);
        let rotated = CursorSecret::with_id(8, [8_u8; 32])
            .with_overlap([(7, [7_u8; 32])])
            .expect("a unique overlap key");
        let old_cursor = encode_cursor(&old, &claims(), 0);
        assert_eq!(
            decode_cursor(&rotated, &old_cursor, &claims(), 0),
            Ok(claims().last)
        );
        let new_cursor = encode_cursor(&rotated, &claims(), 0);
        assert_eq!(
            decode_cursor(&old, &new_cursor, &claims(), 0),
            Err(CursorError::BadSignature)
        );
    }

    #[test]
    fn a_key_ring_is_bounded_and_has_unique_ids() {
        assert!(matches!(
            CursorSecret::with_id(1, [1; 32]).with_overlap([(1, [2; 32])]),
            Err(super::CursorKeyRingError::DuplicateKeyId)
        ));
        assert!(matches!(
            CursorSecret::with_id(1, [1; 32]).with_overlap([
                (2, [2; 32]),
                (3, [3; 32]),
                (4, [4; 32])
            ]),
            Err(super::CursorKeyRingError::TooManyKeys)
        ));
    }

    #[test]
    fn an_unknown_key_id_is_refused_even_when_material_matches() {
        let raw = encode_cursor(&CursorSecret::with_id(99, [7_u8; 32]), &claims(), 0);
        assert_eq!(
            decode_cursor(&secret(), &raw, &claims(), 0),
            Err(CursorError::BadSignature)
        );
    }

    #[test]
    fn an_unknown_envelope_version_and_oversize_wire_value_are_malformed() {
        let raw = encode_cursor(&secret(), &claims(), 0);
        let body = raw.strip_prefix(super::CURSOR_PREFIX).expect("the prefix");
        let (payload, tag) = body.split_once('.').expect("payload and tag");
        let mut bytes = super::unbase64url(payload).expect("the payload");
        bytes[4] = 1;
        let version_one = format!(
            "{}{}.{}",
            super::CURSOR_PREFIX,
            super::base64url(&bytes),
            tag
        );
        assert_eq!(
            decode_cursor(&secret(), &version_one, &claims(), 0),
            Err(CursorError::Malformed)
        );

        let oversize = format!(
            "{}{}",
            super::CURSOR_PREFIX,
            "A".repeat(CURSOR_MAX_ENCODED_BYTES)
        );
        assert!(oversize.len() > CURSOR_MAX_ENCODED_BYTES);
        assert_eq!(
            decode_cursor(&secret(), &oversize, &claims(), 0),
            Err(CursorError::Malformed)
        );
    }

    #[test]
    fn a_malformed_cursor_is_rejected_before_anything_else() {
        for raw in ["", "cur_", "nope_abc.def", "cur_abc", "cur_!!!.???"] {
            assert!(
                matches!(
                    decode_cursor(&secret(), raw, &claims(), 0),
                    Err(CursorError::Malformed | CursorError::BadSignature)
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn the_secret_never_renders() {
        assert_eq!(format!("{:?}", secret()), "<redacted:32 bytes>");
    }
}
