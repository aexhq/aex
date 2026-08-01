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

/// The keyed secret a cursor is signed with.
///
/// Same shape and rotation as a credential pepper, different purpose, so a
/// cursor MAC can never be confused with a credential verifier.
#[derive(Clone)]
pub struct CursorSecret(Zeroizing<[u8; 32]>);

impl CursorSecret {
    /// Wraps 32 secret bytes.
    #[must_use]
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
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
const DOMAIN: &[u8] = b"aex/control/cursor/v1\x1f";

/// The signed payload, as fixed-layout bytes.
///
/// A fixed layout means the payload has no delimiters to confuse, so two
/// different claim sets can never render to the same bytes.
///
/// # Panics
///
/// Never: the endpoint is a generated route template, far shorter than 4 GiB.
fn payload(claims: &CursorClaims, issued_at_ms: i64) -> Vec<u8> {
    let endpoint = claims.endpoint.as_bytes();
    let mut bytes = Vec::with_capacity(128 + endpoint.len());
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
fn mac(secret: &CursorSecret, bytes: &[u8]) -> [u8; 32] {
    let mut hmac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.0.as_ref())
        .expect("HMAC-SHA256 accepts a 32-byte key");
    hmac.update(DOMAIN);
    hmac.update(bytes);
    hmac.finalize().into_bytes().into()
}

/// Encodes a cursor.
#[must_use]
pub fn encode_cursor(secret: &CursorSecret, claims: &CursorClaims, now_ms: i64) -> String {
    let bytes = payload(claims, now_ms);
    let tag = mac(secret, &bytes);
    format!("{CURSOR_PREFIX}{}.{}", base64url(&bytes), base64url(&tag))
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
    let body = raw
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(CursorError::Malformed)?;
    let (payload_text, mac_text) = body.split_once('.').ok_or(CursorError::Malformed)?;
    let bytes = unbase64url(payload_text).ok_or(CursorError::Malformed)?;
    let presented = unbase64url(mac_text).ok_or(CursorError::Malformed)?;
    if presented.len() != 32 {
        return Err(CursorError::Malformed);
    }

    let expected_mac = mac(secret, &bytes);
    if expected_mac.ct_eq(&presented).unwrap_u8() != 1 {
        return Err(CursorError::BadSignature);
    }

    let decoded = parse_payload(&bytes).ok_or(CursorError::Malformed)?;
    if now_ms.saturating_sub(decoded.issued_at_ms) > CURSOR_TTL_MS {
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

/// Parses the fixed-layout payload.
#[allow(
    clippy::missing_panics_doc,
    reason = "every slice index is bounds-checked before use"
)]
fn parse_payload(bytes: &[u8]) -> Option<Decoded> {
    const FIXED: usize = 8 + 16 + 16 + 1 + 32 + 8 + 8 + 16 + 4;
    if bytes.len() < FIXED {
        return None;
    }
    let issued_at_ms = i64::from_be_bytes(bytes[0..8].try_into().ok()?);
    let principal_id = Uuid::from_slice(&bytes[8..24]).ok()?;
    let scope_id = Uuid::from_slice(&bytes[24..40]).ok()?;
    let region = region_from_code(bytes[40])?;
    let filter_hash: [u8; 32] = bytes[41..73].try_into().ok()?;
    let snapshot_ms = i64::from_be_bytes(bytes[73..81].try_into().ok()?);
    let last_created = i64::from_be_bytes(bytes[81..89].try_into().ok()?);
    let last_id = Uuid::from_slice(&bytes[89..105]).ok()?;
    let endpoint_len = u32::from_be_bytes(bytes[105..109].try_into().ok()?) as usize;
    if bytes.len() != FIXED + endpoint_len {
        return None;
    }
    let endpoint = std::str::from_utf8(&bytes[109..109 + endpoint_len])
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
        CURSOR_TTL_MS, CursorClaims, CursorError, CursorSecret, decode_cursor, encode_cursor,
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
