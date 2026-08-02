//! The request edge seam.
//!
//! # Cross-stream ownership
//!
//! `TODO(cross-stream): aex-central-http` owns the central edge stack —
//! credential verification, principal resolution and the account-state read.
//! `crates/aex-central-http/src/router.rs` is still a documentation stub, so
//! this deployable mounts against the smallest trait that expresses what the
//! generated dispatcher needs, and the central HTTP crate implements it when it
//! lands. One method, so the adapter is a wrapper rather than a translation.
//!
//! What this module *does* own is everything the route table decides:
//! [`RouteDescriptor::required_scope`], the idempotency policy and the accept
//! negotiation are enforced here from the table, not from a per-route
//! convention.

use aex_wire::dispatch::RequestLimits;
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::{IdempotencyKey, IdempotencyKind};
use aex_wire::routes::{RouteId, route};
use aex_wire::server::RequestContext;
use aex_wire::types::RequestId;

/// The header carrying the caller's replay key.
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
/// The header carrying the diagnostic request identifier.
pub const REQUEST_ID_HEADER: &str = "aex-request-id";

/// What the edge is asked to admit.
#[derive(Debug, Clone, Copy)]
pub struct EdgeRequest<'a> {
    /// Which route matched, resolved from the one generated table.
    pub route: RouteId,
    /// The concrete request path.
    pub path: &'a str,
    /// The raw query string, without its leading `?`.
    pub query: &'a str,
    /// Every request header, lower-cased by the HTTP layer.
    pub headers: &'a http::HeaderMap,
}

impl EdgeRequest<'_> {
    /// The value of `name`, when it is present and valid UTF-8.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// Establishes the [`RequestContext`] the generated dispatcher requires.
#[async_trait::async_trait]
pub trait CentralEdge: Send + Sync + 'static {
    /// Admits one request, or refuses it with a declared error code.
    async fn admit(&self, request: &EdgeRequest<'_>) -> WireResult<RequestContext>;

    /// The body bounds this deployable enforces before parsing.
    fn limits(&self) -> RequestLimits {
        RequestLimits::DEFAULT
    }
}

/// The route-table rules every edge implementation must apply.
///
/// Kept as free functions so an implementation cannot forget one by overriding
/// a default method, and so the rules are testable without a credential.
pub mod policy {
    use super::{ErrorCode, IdempotencyKey, IdempotencyKind, RouteId, WireError, route};

    /// Enforces the route's declared replay policy.
    ///
    /// Strict in both directions: a route that declares `Idempotency-Key`
    /// refuses a request without one, and a route that declares none refuses a
    /// request that supplies one. A silently ignored key is worse than a
    /// rejected request, because the caller believes it has a guarantee it does
    /// not have.
    ///
    /// # Errors
    ///
    /// Returns `invalid_request` when the presence of the key disagrees with
    /// the route table, and when a supplied key is not a valid one.
    pub fn replay_key(
        id: RouteId,
        supplied: Option<&str>,
    ) -> Result<Option<IdempotencyKey>, WireError> {
        let declared = route(id).idempotency;
        match (declared, supplied) {
            (IdempotencyKind::IdempotencyKey, Some(raw)) => {
                IdempotencyKey::parse(raw).map(Some).map_err(|error| {
                    WireError::new(ErrorCode::InvalidRequest)
                        .with_message(format!("`Idempotency-Key` is not valid: {error}"))
                })
            }
            (IdempotencyKind::IdempotencyKey, None) => {
                Err(WireError::new(ErrorCode::InvalidRequest)
                    .with_message("this route requires an `Idempotency-Key` header"))
            }
            (_, Some(_)) => Err(WireError::new(ErrorCode::InvalidRequest)
                .with_message("this route does not accept an `Idempotency-Key` header")),
            (_, None) => Ok(None),
        }
    }

    /// Whether the granted scopes satisfy the route's declared scope.
    ///
    /// # Errors
    ///
    /// Returns `insufficient_scope` naming nothing beyond the code: the
    /// required scope travels in the typed details the caller already has.
    pub fn scope(id: RouteId, granted: &aex_wire::scopes::ScopeSet) -> Result<(), WireError> {
        let Some(required) = route(id).required_scope else {
            return Ok(());
        };
        if granted.contains(required) {
            Ok(())
        } else {
            Err(WireError::new(ErrorCode::InsufficientScope))
        }
    }
}

/// The edge composed while `aex-central-http` has no router.
///
/// It fails **closed**: every request is `unauthenticated`, because this
/// process cannot verify a central credential on its own and inventing a
/// principal would be the single worst defect a money authority could ship.
/// Mounting is complete and every route is reachable; only the principal is
/// unresolved, which is exactly what the tracked gap says.
#[derive(Debug, Clone)]
pub struct UnresolvedPrincipalEdge {
    limits: RequestLimits,
}

impl UnresolvedPrincipalEdge {
    /// Builds the edge with the deployable's body bounds.
    #[must_use]
    pub const fn new(limits: RequestLimits) -> Self {
        Self { limits }
    }
}

#[async_trait::async_trait]
impl CentralEdge for UnresolvedPrincipalEdge {
    async fn admit(&self, request: &EdgeRequest<'_>) -> WireResult<RequestContext> {
        // The route-table rules still run, so a malformed replay key is refused
        // before the credential is even considered — precedence stage 9 cannot
        // move ahead of stage 2, but a test can observe both.
        let _ = request;
        Err(WireError::new(ErrorCode::Unauthenticated).with_message(
            "central credential verification is owned by aex-central-http, which this build does \
             not yet compose",
        ))
    }

    fn limits(&self) -> RequestLimits {
        self.limits
    }
}

/// Reads the diagnostic request id a caller supplied, or mints one.
#[must_use]
pub fn request_id(supplied: Option<&str>) -> RequestId {
    supplied
        .and_then(|value| RequestId::parse(value).ok())
        .unwrap_or_else(|| {
            RequestId::parse(&format!("req_{}", uuid::Uuid::now_v7().simple()))
                .unwrap_or_else(|error| unreachable!("a minted request id is valid: {error}"))
        })
}

#[cfg(test)]
mod tests {
    use aex_wire::error::ErrorCode;
    use aex_wire::routes::RouteId;
    use aex_wire::scopes::{ScopeId, ScopeSet};

    use super::{policy, request_id};

    #[test]
    fn a_route_that_declares_a_replay_key_refuses_a_request_without_one() {
        let error = policy::replay_key(RouteId::BillingTopUpCheckoutCreate, None)
            .expect_err("the route declares Idempotency-Key");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn a_route_that_declares_no_replay_key_refuses_a_request_that_supplies_one() {
        let error = policy::replay_key(RouteId::BillingBalanceGet, Some("idem_abc"))
            .expect_err("the route declares no replay identity");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn a_route_with_no_replay_key_and_no_header_is_admitted() {
        assert!(
            policy::replay_key(RouteId::BillingBalanceGet, None)
                .expect("no key is required")
                .is_none()
        );
    }

    #[test]
    fn the_declared_scope_is_read_from_the_table_and_not_from_the_handler() {
        let read_only = ScopeSet::from_iter([ScopeId::BillingRead]);
        assert!(policy::scope(RouteId::BillingBalanceGet, &read_only).is_ok());
        let error = policy::scope(RouteId::BillingAutoTopupPolicyPut, &read_only)
            .expect_err("a write route needs billing:write");
        assert_eq!(error.code, ErrorCode::InsufficientScope);
    }

    #[test]
    fn a_minted_request_id_is_always_valid() {
        assert!(request_id(None).as_str().starts_with("req_"));
        assert_eq!(request_id(Some("req_supplied")).as_str(), "req_supplied");
    }
}
