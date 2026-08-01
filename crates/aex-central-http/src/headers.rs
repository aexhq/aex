//! Request-header extraction, strict in both directions.
//!
//! A header the route does not declare is a refusal, not something to ignore. A
//! silently dropped `Idempotency-Key` is worse than a rejected request, because
//! the caller believes it has a replay guarantee it does not have; a silently
//! dropped `If-Match` is worse still, because the caller believes it has a
//! precondition.

use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{OperationId, PrefixedId as _};
use aex_wire::routes::{EtagPolicy, RouteId, route};
use aex_wire::server::AcceptKind;
use aex_wire::types::ETag;
use http::HeaderMap;

use crate::error::EdgeError;

/// The `Idempotency-Key` header.
pub const IDEMPOTENCY_KEY: &str = "idempotency-key";
/// The `Aex-Operation-Id` header.
pub const OPERATION_ID: &str = "aex-operation-id";
/// The `If-Match` header.
pub const IF_MATCH: &str = "if-match";
/// The `Accept` header.
pub const ACCEPT: &str = "accept";

/// What the declared headers of one request resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredHeaders {
    /// The caller-chosen replay key.
    pub idempotency_key: Option<IdempotencyKey>,
    /// The caller-minted durable-operation id.
    pub operation_id: Option<OperationId>,
    /// The precondition the caller supplied.
    pub if_match: Option<ETag>,
    /// What the caller said it will accept.
    pub accept: AcceptKind,
}

/// Reads one ASCII header value.
fn value<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<Option<&'a str>, EdgeError> {
    match headers.get(name) {
        None => Ok(None),
        Some(raw) => raw
            .to_str()
            .map(Some)
            .map_err(|_| EdgeError::InvalidRequest(format!("`{name}` is not ASCII"))),
    }
}

/// Resolves every declared header of `id`.
///
/// # Errors
///
/// Returns [`EdgeError::InvalidRequest`] when a header the route requires is
/// absent, a header the route declares none of is present, or a value does not
/// parse.
pub fn declared(id: RouteId, headers: &HeaderMap) -> Result<DeclaredHeaders, EdgeError> {
    let descriptor = route(id);

    let idempotency_key = value(headers, IDEMPOTENCY_KEY)?
        .map(|raw| {
            IdempotencyKey::parse(raw).map_err(|reason| {
                EdgeError::InvalidRequest(format!("`Idempotency-Key` was rejected: {reason}"))
            })
        })
        .transpose()?;
    let operation_id = value(headers, OPERATION_ID)?
        .map(|raw| {
            OperationId::parse(raw).map_err(|reason| {
                EdgeError::InvalidRequest(format!("`Aex-Operation-Id` was rejected: {reason}"))
            })
        })
        .transpose()?;
    let if_match = value(headers, IF_MATCH)?
        .map(|raw| {
            ETag::parse(raw).map_err(|reason| {
                EdgeError::InvalidRequest(format!("`If-Match` was rejected: {reason}"))
            })
        })
        .transpose()?;

    match descriptor.etag {
        EtagPolicy::RequiredIfMatch if if_match.is_none() => {
            return Err(EdgeError::InvalidRequest(format!(
                "`{}` requires an `If-Match`",
                descriptor.operation_id
            )));
        }
        EtagPolicy::None if if_match.is_some() => {
            return Err(EdgeError::InvalidRequest(format!(
                "`{}` does not accept an `If-Match`",
                descriptor.operation_id
            )));
        }
        EtagPolicy::None
        | EtagPolicy::Returns
        | EtagPolicy::OptionalIfMatch
        | EtagPolicy::RequiredIfMatch => {}
    }

    Ok(DeclaredHeaders {
        idempotency_key,
        operation_id,
        if_match,
        accept: accept(headers)?,
    })
}

/// What the caller said it will accept.
///
/// An absent `Accept` means JSON, which is what every central route returns; a
/// present one that excludes JSON is a refusal rather than a JSON body the
/// caller said it would not read.
fn accept(headers: &HeaderMap) -> Result<AcceptKind, EdgeError> {
    let Some(raw) = value(headers, ACCEPT)? else {
        return Ok(AcceptKind::Json);
    };
    let admits = raw.split(',').map(str::trim).any(|media| {
        let media = media.split(';').next().unwrap_or_default().trim();
        matches!(media, "application/json" | "application/*" | "*/*" | "")
    });
    if admits {
        Ok(AcceptKind::Json)
    } else {
        Err(EdgeError::InvalidRequest(
            "the central plane answers `application/json` only".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{ACCEPT, IDEMPOTENCY_KEY, IF_MATCH, OPERATION_ID, declared};
    use crate::error::EdgeError;
    use aex_wire::routes::RouteId;
    use aex_wire::server::AcceptKind;
    use http::{HeaderMap, HeaderValue};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).expect("a valid header"));
        }
        map
    }

    #[test]
    fn a_replay_key_is_read_where_the_route_declares_one() {
        let resolved = declared(
            RouteId::OrganizationCreate,
            &headers(&[(IDEMPOTENCY_KEY, "key-1")]),
        )
        .expect("the header is declared");
        assert_eq!(
            resolved.idempotency_key.map(|key| key.as_str().to_owned()),
            Some("key-1".to_owned())
        );
        assert_eq!(resolved.accept, AcceptKind::Json);
    }

    #[test]
    fn a_required_if_match_is_refused_when_absent() {
        let error = declared(RouteId::BillingAutoTopupPolicyPut, &HeaderMap::new())
            .expect_err("the route requires `If-Match`");
        assert!(matches!(error, EdgeError::InvalidRequest(_)), "{error:?}");
    }

    #[test]
    fn an_if_match_on_a_route_with_no_etag_policy_is_refused() {
        let error = declared(
            RouteId::OrganizationCreate,
            &headers(&[(IDEMPOTENCY_KEY, "key-1"), (IF_MATCH, "\"1\"")]),
        )
        .expect_err("the route declares no entity tag");
        assert!(matches!(error, EdgeError::InvalidRequest(_)), "{error:?}");
    }

    #[test]
    fn an_optional_if_match_is_accepted_both_ways() {
        assert!(declared(RouteId::ApiKeyRevoke, &HeaderMap::new()).is_ok());
        assert!(declared(RouteId::ApiKeyRevoke, &headers(&[(IF_MATCH, "\"7\"")])).is_ok());
    }

    #[test]
    fn a_malformed_operation_id_is_a_four_hundred_rather_than_a_dropped_header() {
        let error = declared(
            RouteId::WorkspaceDelete,
            &headers(&[(OPERATION_ID, "nope")]),
        )
        .expect_err("the id does not parse");
        assert!(matches!(error, EdgeError::InvalidRequest(_)), "{error:?}");
    }

    #[test]
    fn an_accept_that_excludes_json_is_refused() {
        let error = declared(
            RouteId::OrganizationsList,
            &headers(&[(ACCEPT, "application/xml")]),
        )
        .expect_err("the central plane answers JSON only");
        assert!(matches!(error, EdgeError::InvalidRequest(_)), "{error:?}");

        for admitted in [
            "*/*",
            "application/json",
            "application/json; q=0.9",
            "text/html, */*",
        ] {
            assert!(
                declared(RouteId::OrganizationsList, &headers(&[(ACCEPT, admitted)])).is_ok(),
                "{admitted}"
            );
        }
    }

    #[test]
    fn a_non_ascii_header_is_refused_before_it_is_parsed() {
        let mut map = HeaderMap::new();
        map.insert(
            IDEMPOTENCY_KEY,
            http::HeaderValue::from_bytes(&[0xff, 0xfe]).expect("bytes"),
        );
        let error =
            declared(RouteId::OrganizationCreate, &map).expect_err("a non-ASCII value is refused");
        assert!(matches!(error, EdgeError::InvalidRequest(_)), "{error:?}");
    }
}
