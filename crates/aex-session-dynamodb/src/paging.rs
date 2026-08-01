//! Bounded paging and the signed continuation token.
//!
//! A cursor is an opaque bearer value that names a position inside somebody's
//! data, so it is signed and bound. Binding it to the resource, the organization
//! and the workspace is what stops a cursor minted for one collection from
//! being replayed against another; the 24-hour snapshot bound is what stops one
//! from outliving the query it belongs to.

use std::collections::HashMap;

use aex_wire::canonical;
use aex_wire::cursor::Cursor;
use aex_wire::ids::{OrganizationId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::AttributeValue;
use base64::Engine as _;
use hmac::{KeyInit as _, Mac as _};
use serde::{Deserialize, Serialize};

use crate::attr::{PK, SK};

/// The `cursor.snapshot` bound: a continuation is valid for 24 hours.
pub const SNAPSHOT_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// The smallest signing key that may be used.
pub const MIN_KEY_BYTES: usize = 32;

/// The largest page a caller may request.
pub const MAX_PAGE_ITEMS: u32 = 100;

/// The default page size when the caller names none.
pub const DEFAULT_PAGE_ITEMS: u32 = 25;

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// Why a cursor could not be minted or accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    /// The signing key was shorter than [`MIN_KEY_BYTES`].
    #[error("a cursor signing key is {found} bytes; at least {MIN_KEY_BYTES} are required")]
    WeakKey {
        /// The key length that was offered.
        found: usize,
    },
    /// The envelope was not the expected grammar.
    #[error("the cursor envelope is malformed: {reason}")]
    Malformed {
        /// What was wrong.
        reason: &'static str,
    },
    /// The signature did not verify under this binding.
    ///
    /// Reported identically whether the token was tampered with or simply
    /// minted for another collection: distinguishing them would tell a prober
    /// which of the two it got wrong.
    #[error("the cursor is not valid for this collection")]
    NotBound,
    /// The cursor is older than the snapshot bound.
    #[error("the cursor expired; a listing continuation is valid for 24 hours")]
    Expired,
    /// The caller asked for more items than a page may hold.
    #[error("a page of {found} items was requested; the maximum is {MAX_PAGE_ITEMS}")]
    PageTooLarge {
        /// What was requested.
        found: u32,
    },
}

/// What a cursor is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorBinding<'a> {
    /// A stable name for the collection, for example `sessions` or
    /// `session.messages`.
    pub resource: &'a str,
    /// The owning organization.
    pub organization: OrganizationId,
    /// The workspace.
    pub workspace: WorkspaceId,
}

/// The position a cursor resumes from.
///
/// Only key attributes appear here, so a cursor can never carry a prompt, a
/// body or a receipt even if the row it points at does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PagePosition {
    /// The base table partition key.
    pub pk: String,
    /// The base table sort key.
    pub sk: String,
    /// The index partition key, when the query ran over an index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_pk: Option<String>,
    /// The index sort key, when the query ran over an index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_sk: Option<String>,
}

impl PagePosition {
    /// Reads a position out of a `DynamoDB` `LastEvaluatedKey`.
    ///
    /// # Errors
    ///
    /// [`CursorError::Malformed`] when the service returned a key without the
    /// base table's own attributes, which would make the continuation
    /// unresumable.
    #[allow(
        clippy::similar_names,
        reason = "`index_pk` and `index_sk` are the provider's own attribute roles"
    )]
    pub fn from_last_evaluated(
        key: &HashMap<String, AttributeValue>,
        index_pk: Option<&str>,
        index_sk: Option<&str>,
    ) -> Result<Self, CursorError> {
        let read = |name: &str| -> Option<String> {
            key.get(name).and_then(|value| value.as_s().ok()).cloned()
        };
        let (Some(pk), Some(sk)) = (read(PK), read(SK)) else {
            return Err(CursorError::Malformed {
                reason: "the last evaluated key carries no base table key",
            });
        };
        Ok(Self {
            pk,
            sk,
            index_pk: index_pk.and_then(read),
            index_sk: index_sk.and_then(read),
        })
    }

    /// Renders the position as a `DynamoDB` `ExclusiveStartKey`.
    #[must_use]
    #[allow(
        clippy::similar_names,
        reason = "`index_pk` and `index_sk` are the provider's own attribute roles"
    )]
    pub fn to_exclusive_start(
        &self,
        index_pk: Option<&str>,
        index_sk: Option<&str>,
    ) -> HashMap<String, AttributeValue> {
        let mut key = HashMap::from([
            (PK.to_owned(), crate::attr::s(self.pk.clone())),
            (SK.to_owned(), crate::attr::s(self.sk.clone())),
        ]);
        if let (Some(name), Some(value)) = (index_pk, self.index_pk.as_ref()) {
            key.insert(name.to_owned(), crate::attr::s(value.clone()));
        }
        if let (Some(name), Some(value)) = (index_sk, self.index_sk.as_ref()) {
            key.insert(name.to_owned(), crate::attr::s(value.clone()));
        }
        key
    }
}

/// The signed payload, canonicalized before signing so the bytes a verifier
/// hashes are exactly the bytes a minter hashed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Payload {
    /// Envelope version. A future format change is a new number, never a
    /// tolerated parse.
    v: u8,
    r: String,
    o: String,
    w: String,
    /// Issue time in Unix milliseconds.
    i: i64,
    p: PagePosition,
}

/// The cursor signing key.
///
/// Held as bytes rather than a string so a configuration mistake that supplies
/// a short key fails at construction and not at the first list request.
#[derive(Clone)]
pub struct CursorKey(Vec<u8>);

impl CursorKey {
    /// Wraps signing key material.
    ///
    /// # Errors
    ///
    /// [`CursorError::WeakKey`] below [`MIN_KEY_BYTES`].
    pub fn new(material: impl Into<Vec<u8>>) -> Result<Self, CursorError> {
        let material = material.into();
        if material.len() < MIN_KEY_BYTES {
            return Err(CursorError::WeakKey {
                found: material.len(),
            });
        }
        Ok(Self(material))
    }
}

impl std::fmt::Debug for CursorKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CursorKey(<redacted>)")
    }
}

/// Mints a signed continuation.
///
/// # Errors
///
/// [`CursorError::Malformed`] only if the payload cannot be canonicalized,
/// which would be a defect in this module rather than a data condition.
pub fn mint(
    key: &CursorKey,
    binding: CursorBinding<'_>,
    position: &PagePosition,
    issued_at: Timestamp,
) -> Result<Cursor, CursorError> {
    let payload = Payload {
        v: 1,
        r: binding.resource.to_owned(),
        o: binding.organization.to_string(),
        w: binding.workspace.to_string(),
        i: issued_at.unix_millis(),
        p: position.clone(),
    };
    let bytes = canonical::to_jcs_bytes(&payload).map_err(|_| CursorError::Malformed {
        reason: "the cursor payload is not canonicalizable",
    })?;
    let tag = sign(key, &bytes);
    let mut envelope = Vec::with_capacity(tag.len() + bytes.len());
    envelope.extend_from_slice(&tag);
    envelope.extend_from_slice(&bytes);
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope);
    Cursor::parse(&format!("{}{body}", Cursor::PREFIX)).map_err(|_| CursorError::Malformed {
        reason: "the minted cursor is not a valid envelope",
    })
}

/// Verifies a continuation and returns the position it names.
///
/// # Errors
///
/// [`CursorError::Malformed`] for a broken envelope, [`CursorError::NotBound`]
/// when the signature does not verify under this binding, and
/// [`CursorError::Expired`] past the snapshot bound.
pub fn verify(
    key: &CursorKey,
    binding: CursorBinding<'_>,
    cursor: &Cursor,
    now: Timestamp,
) -> Result<PagePosition, CursorError> {
    let body = cursor
        .as_str()
        .strip_prefix(Cursor::PREFIX)
        .ok_or(CursorError::Malformed {
            reason: "missing envelope prefix",
        })?;
    let envelope = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| CursorError::Malformed {
            reason: "the envelope body is not base64url",
        })?;
    if envelope.len() <= 32 {
        return Err(CursorError::Malformed {
            reason: "the envelope is shorter than its signature",
        });
    }
    let (tag, bytes) = envelope.split_at(32);

    // Verify before parsing: a payload that has not been authenticated is
    // attacker-controlled input to the parser.
    let expected = sign(key, bytes);
    if !constant_time_eq(tag, &expected) {
        return Err(CursorError::NotBound);
    }
    let payload: Payload = serde_json::from_slice(bytes).map_err(|_| CursorError::Malformed {
        reason: "the signed payload is not a cursor",
    })?;
    if payload.v != 1 {
        return Err(CursorError::Malformed {
            reason: "unknown cursor envelope version",
        });
    }
    if payload.r != binding.resource
        || payload.o != binding.organization.to_string()
        || payload.w != binding.workspace.to_string()
    {
        return Err(CursorError::NotBound);
    }
    if now.unix_millis().saturating_sub(payload.i) > SNAPSHOT_MILLIS {
        return Err(CursorError::Expired);
    }
    Ok(payload.p)
}

fn sign(key: &CursorKey, bytes: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(&key.0).expect("HMAC accepts any key length");
    mac.update(bytes);
    mac.finalize().into_bytes().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |accumulator, (a, b)| accumulator | (a ^ b))
        == 0
}

/// A validated page size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBudget(u32);

impl PageBudget {
    /// Validates a requested page size.
    ///
    /// # Errors
    ///
    /// [`CursorError::PageTooLarge`] above [`MAX_PAGE_ITEMS`]. The request is
    /// rejected rather than silently clamped, so a caller is never told it
    /// received everything when it did not.
    pub fn new(requested: u32) -> Result<Self, CursorError> {
        if requested == 0 {
            return Ok(Self(DEFAULT_PAGE_ITEMS));
        }
        if requested > MAX_PAGE_ITEMS {
            return Err(CursorError::PageTooLarge { found: requested });
        }
        Ok(Self(requested))
    }

    /// The item budget.
    #[must_use]
    pub const fn items(self) -> u32 {
        self.0
    }

    /// The budget as the SDK's signed limit.
    #[must_use]
    #[allow(
        clippy::cast_possible_wrap,
        reason = "the budget is bounded by MAX_PAGE_ITEMS, far below i32::MAX"
    )]
    pub const fn limit(self) -> i32 {
        self.0 as i32
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OrganizationId, PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        CursorBinding, CursorError, CursorKey, MAX_PAGE_ITEMS, MIN_KEY_BYTES, PageBudget,
        PagePosition, SNAPSHOT_MILLIS, mint, verify,
    };

    fn key() -> CursorKey {
        CursorKey::new(vec![7u8; MIN_KEY_BYTES]).expect("a long enough key")
    }

    fn organization(byte: u8) -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1, [byte; 10]))
    }

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(2, [byte; 10]))
    }

    fn binding(resource: &str, org: u8, ws: u8) -> CursorBinding<'_> {
        CursorBinding {
            resource,
            organization: organization(org),
            workspace: workspace(ws),
        }
    }

    fn position() -> PagePosition {
        PagePosition {
            pk: "SESSION#ses_1".to_owned(),
            sk: "MSG#msg_1".to_owned(),
            index_pk: None,
            index_sk: None,
        }
    }

    fn stamp(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    #[test]
    fn a_minted_cursor_round_trips_under_its_own_binding() {
        let cursor =
            mint(&key(), binding("sessions", 1, 1), &position(), stamp(1_000)).expect("mints");
        assert!(cursor.as_str().starts_with("cur_"));
        let resumed =
            verify(&key(), binding("sessions", 1, 1), &cursor, stamp(2_000)).expect("verifies");
        assert_eq!(resumed, position());
    }

    #[test]
    fn a_cursor_minted_for_another_workspace_is_refused() {
        let cursor = mint(&key(), binding("sessions", 1, 1), &position(), stamp(0)).expect("mints");
        assert_eq!(
            verify(&key(), binding("sessions", 1, 2), &cursor, stamp(0)).unwrap_err(),
            CursorError::NotBound
        );
    }

    #[test]
    fn a_cursor_minted_for_another_organization_is_refused() {
        let cursor = mint(&key(), binding("sessions", 1, 1), &position(), stamp(0)).expect("mints");
        assert_eq!(
            verify(&key(), binding("sessions", 2, 1), &cursor, stamp(0)).unwrap_err(),
            CursorError::NotBound
        );
    }

    #[test]
    fn a_cursor_minted_for_another_collection_is_refused() {
        let cursor = mint(&key(), binding("sessions", 1, 1), &position(), stamp(0)).expect("mints");
        assert_eq!(
            verify(&key(), binding("operations", 1, 1), &cursor, stamp(0)).unwrap_err(),
            CursorError::NotBound
        );
    }

    #[test]
    fn a_cursor_signed_with_another_key_is_refused() {
        let cursor = mint(&key(), binding("sessions", 1, 1), &position(), stamp(0)).expect("mints");
        let other = CursorKey::new(vec![9u8; MIN_KEY_BYTES]).expect("a key");
        assert_eq!(
            verify(&other, binding("sessions", 1, 1), &cursor, stamp(0)).unwrap_err(),
            CursorError::NotBound
        );
    }

    #[test]
    fn a_tampered_payload_fails_before_it_is_parsed() {
        let cursor = mint(&key(), binding("sessions", 1, 1), &position(), stamp(0)).expect("mints");
        let mut text = cursor.as_str().to_owned();
        let last = text.pop().expect("a body");
        text.push(if last == 'A' { 'B' } else { 'A' });
        let tampered = aex_wire::cursor::Cursor::parse(&text).expect("still an envelope");
        let error = verify(&key(), binding("sessions", 1, 1), &tampered, stamp(0)).unwrap_err();
        assert!(
            matches!(error, CursorError::NotBound | CursorError::Malformed { .. }),
            "{error}"
        );
    }

    #[test]
    fn the_snapshot_bound_is_exact_at_the_boundary() {
        let cursor = mint(&key(), binding("sessions", 1, 1), &position(), stamp(0)).expect("mints");
        assert!(
            verify(
                &key(),
                binding("sessions", 1, 1),
                &cursor,
                stamp(SNAPSHOT_MILLIS)
            )
            .is_ok()
        );
        assert_eq!(
            verify(
                &key(),
                binding("sessions", 1, 1),
                &cursor,
                stamp(SNAPSHOT_MILLIS + 1)
            )
            .unwrap_err(),
            CursorError::Expired
        );
    }

    #[test]
    fn a_short_signing_key_is_refused_at_construction() {
        assert_eq!(
            CursorKey::new(vec![0u8; MIN_KEY_BYTES - 1]).unwrap_err(),
            CursorError::WeakKey {
                found: MIN_KEY_BYTES - 1
            }
        );
    }

    #[test]
    fn the_signing_key_never_prints_its_material() {
        let rendered = format!("{:?}", key());
        assert_eq!(rendered, "CursorKey(<redacted>)");
    }

    #[test]
    fn an_over_large_page_is_refused_rather_than_clamped() {
        assert_eq!(PageBudget::new(0).expect("default").items(), 25);
        assert_eq!(
            PageBudget::new(MAX_PAGE_ITEMS)
                .expect("the maximum")
                .items(),
            MAX_PAGE_ITEMS
        );
        assert_eq!(
            PageBudget::new(MAX_PAGE_ITEMS + 1).unwrap_err(),
            CursorError::PageTooLarge {
                found: MAX_PAGE_ITEMS + 1
            }
        );
    }
}
