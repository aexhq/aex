//! The continuation-cursor boundary.
//!
//! This is where the opaque `cur_` token becomes typed keyset claims. The Aurora
//! adapter deliberately refuses an undecoded cursor, because there are only two
//! other options and both are wrong: ignoring it repeats the first page, and
//! decoding it without the signing secret accepts attacker-controlled ordering
//! state.
//!
//! Every field the query was bound by is inside the signed payload, so a cursor
//! minted for one principal, endpoint, scope or region fails the comparison
//! rather than paging over rows its holder may not see.

use aex_control_app::ports::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT, PageRequest};
use aex_control_domain::{CursorClaims, CursorSecret, decode_cursor, encode_cursor};
use aex_wire::types::Region;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::error::EdgeError;

/// What a page read is bound to, before the last row is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBinding {
    /// The canonical route template the page belongs to.
    pub endpoint: &'static str,
    /// Which principal is paging.
    pub principal_id: Uuid,
    /// The organization or workspace the query is scoped to.
    pub scope_id: Uuid,
    /// The region the query runs in.
    pub region: Region,
    /// A digest over every filter the query applies.
    pub filter_hash: [u8; 32],
    /// When the first page of this sequence was taken.
    pub snapshot_ms: i64,
}

impl PageBinding {
    /// The digest of an ordered filter list.
    ///
    /// Length-prefixed, so two filter lists cannot collide by moving a boundary
    /// between adjacent values.
    #[must_use]
    pub fn filter_hash(filters: &[(&str, &str)]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"aex/central/page-filter/v1\x1f");
        for (name, value) in filters {
            hasher.update(u32::try_from(name.len()).unwrap_or(u32::MAX).to_be_bytes());
            hasher.update(name.as_bytes());
            hasher.update(u32::try_from(value.len()).unwrap_or(u32::MAX).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        hasher.finalize().into()
    }

    /// The claim set this binding produces for `last`.
    fn claims(&self, last: (i64, Uuid)) -> CursorClaims {
        CursorClaims {
            endpoint: self.endpoint.to_owned(),
            principal_id: self.principal_id,
            scope_id: self.scope_id,
            region: self.region,
            filter_hash: self.filter_hash,
            snapshot_ms: self.snapshot_ms,
            last,
        }
    }
}

/// Turns a caller's `cursor`/`limit` pair into a typed keyset page request.
///
/// A cursor is verified **before** any binding is compared, so a forged one
/// learns nothing about what the real bindings would have been.
///
/// # Errors
///
/// Returns [`EdgeError::InvalidCursor`] for a cursor that is malformed, does not
/// verify, has lapsed, or was minted for a different query.
pub fn page_request(
    secret: &CursorSecret,
    binding: &PageBinding,
    cursor: Option<&str>,
    limit: Option<u32>,
    now_ms: i64,
) -> Result<PageRequest, EdgeError> {
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
    let Some(raw) = cursor else {
        return Ok(PageRequest { after: None, limit });
    };
    // The position a cursor carries is what the caller is asking for, so it is
    // not a binding: `decode_cursor` compares the five fields the query fixes
    // and returns the recorded position.
    let expected = binding.claims((0, Uuid::nil()));
    decode_cursor(secret, raw, &expected, now_ms)
        .map(|last| PageRequest {
            after: Some(last),
            limit,
        })
        .map_err(|_| EdgeError::InvalidCursor)
}

/// Mints the cursor for the next page, when the read filled its limit.
///
/// A page that did not fill its limit has no next page, and minting a cursor for
/// it would tell a caller to make a request that can only ever be empty.
#[must_use]
pub fn next_cursor(
    secret: &CursorSecret,
    binding: &PageBinding,
    last: Option<(i64, Uuid)>,
    now_ms: i64,
) -> Option<String> {
    last.map(|last| encode_cursor(secret, &binding.claims(last), now_ms))
}

#[cfg(test)]
mod tests {
    use super::{PageBinding, next_cursor, page_request};
    use crate::error::EdgeError;
    use aex_control_app::ports::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
    use aex_control_domain::CursorSecret;
    use aex_wire::types::Region;
    use uuid::Uuid;

    fn secret() -> CursorSecret {
        CursorSecret::new([7_u8; 32])
    }

    fn binding() -> PageBinding {
        PageBinding {
            endpoint: "/api/organizations",
            principal_id: Uuid::from_u128(1),
            scope_id: Uuid::from_u128(2),
            region: Region::EuWest1,
            filter_hash: PageBinding::filter_hash(&[("status", "active")]),
            snapshot_ms: 1_000,
        }
    }

    #[test]
    fn a_first_page_carries_no_position_and_a_bounded_limit() {
        let page = page_request(&secret(), &binding(), None, None, 2_000).expect("a first page");
        assert_eq!(page.after, None);
        assert_eq!(
            page.limit, DEFAULT_PAGE_LIMIT,
            "an omitted limit is the default, never the ceiling"
        );

        let page = page_request(&secret(), &binding(), None, Some(0), 2_000).expect("a first page");
        assert_eq!(page.limit, 1, "a zero limit is clamped, never zero rows");

        let page =
            page_request(&secret(), &binding(), None, Some(9_999), 2_000).expect("a first page");
        assert_eq!(page.limit, MAX_PAGE_LIMIT);

        let page = page_request(&secret(), &binding(), None, Some(MAX_PAGE_LIMIT), 2_000)
            .expect("a first page");
        assert_eq!(
            page.limit, MAX_PAGE_LIMIT,
            "the ceiling stays reachable on explicit request"
        );
    }

    #[test]
    fn a_minted_cursor_round_trips_into_its_recorded_position() {
        let last = (1_234_i64, Uuid::from_u128(9));
        let cursor = next_cursor(&secret(), &binding(), Some(last), 1_000).expect("a next page");
        let page = page_request(&secret(), &binding(), Some(&cursor), Some(10), 2_000)
            .expect("the cursor verifies");
        assert_eq!(page.after, Some(last));
        assert_eq!(page.limit, 10);
    }

    #[test]
    fn a_cursor_minted_for_another_principal_is_refused() {
        let cursor = next_cursor(&secret(), &binding(), Some((1, Uuid::from_u128(9))), 1_000)
            .expect("a next page");
        let mut other = binding();
        other.principal_id = Uuid::from_u128(42);
        assert_eq!(
            page_request(&secret(), &other, Some(&cursor), None, 2_000),
            Err(EdgeError::InvalidCursor)
        );
    }

    #[test]
    fn a_cursor_minted_for_another_endpoint_or_filter_is_refused() {
        let cursor = next_cursor(&secret(), &binding(), Some((1, Uuid::from_u128(9))), 1_000)
            .expect("a next page");
        let mut other = binding();
        other.endpoint = "/api/workspaces";
        assert_eq!(
            page_request(&secret(), &other, Some(&cursor), None, 2_000),
            Err(EdgeError::InvalidCursor)
        );

        let mut other = binding();
        other.filter_hash = PageBinding::filter_hash(&[("status", "deleted")]);
        assert_eq!(
            page_request(&secret(), &other, Some(&cursor), None, 2_000),
            Err(EdgeError::InvalidCursor)
        );
    }

    #[test]
    fn a_cursor_signed_with_another_secret_is_refused() {
        let cursor = next_cursor(&secret(), &binding(), Some((1, Uuid::from_u128(9))), 1_000)
            .expect("a next page");
        assert_eq!(
            page_request(
                &CursorSecret::new([8_u8; 32]),
                &binding(),
                Some(&cursor),
                None,
                2_000
            ),
            Err(EdgeError::InvalidCursor)
        );
    }

    #[test]
    fn a_lapsed_cursor_is_refused_rather_than_silently_restarting() {
        let cursor = next_cursor(&secret(), &binding(), Some((1, Uuid::from_u128(9))), 1_000)
            .expect("a next page");
        let a_day_later = 1_000 + 24 * 60 * 60 * 1000 + 1;
        assert_eq!(
            page_request(&secret(), &binding(), Some(&cursor), None, a_day_later),
            Err(EdgeError::InvalidCursor)
        );
    }

    #[test]
    fn a_garbage_cursor_is_refused() {
        for raw in ["", "cur_", "not-a-cursor", "cur_zzzz.zzzz"] {
            assert_eq!(
                page_request(&secret(), &binding(), Some(raw), None, 2_000),
                Err(EdgeError::InvalidCursor),
                "{raw}"
            );
        }
    }

    #[test]
    fn an_exhausted_page_mints_no_cursor() {
        assert_eq!(next_cursor(&secret(), &binding(), None, 1_000), None);
    }

    #[test]
    fn the_filter_digest_is_length_prefixed_so_a_boundary_cannot_shift() {
        assert_ne!(
            PageBinding::filter_hash(&[("ab", "c")]),
            PageBinding::filter_hash(&[("a", "bc")])
        );
    }
}
