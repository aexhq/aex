//! Regional replay identities derived from canonical request bytes.

use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{OperationId, PrefixedId as _, WorkspaceId};
use aex_wire::routes::RouteId;
use aex_wire::types::HttpMethod;
use http::HeaderMap;
use sha2::Digest as _;

/// Stable request facts used to derive an idempotency scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityContext<'a> {
    /// Stable principal identifier, never credential material.
    pub principal: &'a str,
    /// Organization identifier.
    pub organization: &'a str,
    /// Workspace identifier.
    pub workspace: WorkspaceId,
    /// Generated route identity.
    pub route: RouteId,
    /// HTTP method from generated metadata.
    pub method: HttpMethod,
}

/// The replay lookup key and the canonical intent it protects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyIdentity {
    /// Digest of principal, authority, method and route.
    pub scope: [u8; 32],
    /// Caller-provided replay key.
    pub key: IdempotencyKey,
    /// Digest of the scope and the already-canonicalized request bytes.
    pub intent: [u8; 32],
}

/// Constructs a replay identity. `canonical` must come from `aex_wire::canonical`.
///
/// The intent is `sha256(scope_digest ‖ canonical_bytes)`, not a bare digest of
/// the body, and it is derived that way for **every** route rather than for the
/// ones whose bodies happen to be sensitive. An unsalted body digest stored in a
/// durable receipt is a rainbow-table target that one precomputed table covers
/// fleet-wide — `secret_put`'s canonical body is `{"value":"<the secret>"}` — and
/// the salt costs nothing, because the scope is already a fixed-width digest and
/// the result is still a perfect equality test.
#[must_use]
pub fn identity(
    context: &IdentityContext<'_>,
    key: &IdempotencyKey,
    canonical: &[u8],
) -> IdempotencyIdentity {
    let scope = digest_fields(&[
        context.principal.as_bytes(),
        context.organization.as_bytes(),
        context.workspace.to_string().as_bytes(),
        context.method.as_str().as_bytes(),
        context.route.as_str().as_bytes(),
    ]);
    let intent = sha2::Sha256::new()
        .chain_update(scope)
        .chain_update(canonical)
        .finalize()
        .into();
    IdempotencyIdentity {
        scope,
        key: key.clone(),
        intent,
    }
}

/// Why the operation-identity header was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// Header was absent.
    #[error("Aex-Operation-Id is required")]
    MissingOperationId,
    /// Header was not visible ASCII or not a valid `op_` id.
    #[error("Aex-Operation-Id is invalid")]
    InvalidOperationId,
}

/// Parses the sole carrier of durable operation identity.
///
/// # Errors
///
/// Returns [`IdentityError`] when the header is absent or not an `op_` id.
pub fn operation_id(headers: &HeaderMap) -> Result<OperationId, IdentityError> {
    let value = headers
        .get("Aex-Operation-Id")
        .ok_or(IdentityError::MissingOperationId)?
        .to_str()
        .map_err(|_| IdentityError::InvalidOperationId)?;
    OperationId::parse(value).map_err(|_| IdentityError::InvalidOperationId)
}

fn digest_fields(fields: &[&[u8]]) -> [u8; 32] {
    let mut digest = sha2::Sha256::new();
    for field in fields {
        let length = u64::try_from(field.len()).expect("an in-memory field length fits u64");
        digest.update(length.to_be_bytes());
        digest.update(field);
    }
    digest.finalize().into()
}
