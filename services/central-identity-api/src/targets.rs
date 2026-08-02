//! The target resolver for a deployable that serves no organization-scoped
//! route.
//!
//! `central-identity-api` mounts exactly `RouteGroup::Auth`: two anonymous
//! device-flow routes, both `pause_exempt`, both actor-scoped. Neither names a
//! resource and neither reads an account state, which
//! `aex_central_http::target::admit_request` enforces by consulting the route's
//! own requirement before it touches this resolver.
//!
//! So both methods are refusals rather than lookups. That is not a stub: this
//! binary's login role is `aex_identity_api`, which holds no privilege on the
//! `control` schema at all. A resolver that answered would have to read a table
//! it cannot select from, and one that *guessed* — returning `Active` for any
//! organization — would admit a paused account on whatever route later reached
//! it. Refusing is the only answer consistent with the privilege the role holds.

use aex_central_http::error::EdgeError;
use aex_central_http::target::{TargetPath, TargetResolver};
use aex_control_domain::{AccountState, Resource};
use aex_wire::routes::RouteId;
use async_trait::async_trait;
use uuid::Uuid;

/// The resolver for the two anonymous device-flow routes.
#[derive(Debug, Clone, Copy)]
pub struct NoOrganizationTargets;

#[async_trait]
impl TargetResolver for NoOrganizationTargets {
    async fn resolve(
        &self,
        _route: RouteId,
        _path: &TargetPath,
    ) -> Result<Option<Resource>, EdgeError> {
        // Reached only if a resource-class route is ever mounted here. `None`
        // renders as `404`, which is the correct answer for a resource this
        // deployable cannot see.
        Ok(None)
    }

    async fn account_state(&self, _organization_id: Uuid) -> Result<AccountState, EdgeError> {
        // Unreachable for every mounted route, and a hard refusal if that ever
        // stops being true. `AccountStateUnavailable` renders as a retryable
        // `503`; answering `Active` would be a guess that admits a paused
        // account.
        Err(EdgeError::AccountStateUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::NoOrganizationTargets;
    use aex_central_http::config::CentralServiceId;
    use aex_central_http::error::EdgeError;
    use aex_central_http::target::{TargetPath, TargetResolver as _};
    use aex_control_domain::{Action, requirement};
    use uuid::Uuid;

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime")
            .block_on(future)
    }

    #[test]
    fn no_mounted_route_needs_a_resource_or_an_account_state() {
        for id in CentralServiceId::IdentityApi.routes() {
            let action = Action::central(id).expect("a central route");
            let requirement = requirement(action);
            assert_eq!(
                requirement.resource_class,
                aex_control_domain::ResourceClass::None,
                "`{id:?}` names a resource this deployable cannot read"
            );
            assert!(
                requirement.pause_exempt,
                "`{id:?}` would read an account state this role has no privilege for"
            );
        }
    }

    #[test]
    fn an_account_state_read_is_refused_rather_than_guessed() {
        let error = run(NoOrganizationTargets.account_state(Uuid::nil()))
            .expect_err("this role holds no control privilege");
        assert_eq!(error, EdgeError::AccountStateUnavailable);
    }

    #[test]
    fn an_unreachable_resource_is_absent_rather_than_fabricated() {
        let resolved = run(NoOrganizationTargets.resolve(
            aex_wire::routes::RouteId::DeviceAuthorizationCreate,
            &TargetPath::from_binding(&aex_wire::routes::PathBinding::default()),
        ))
        .expect("the resolver answers");
        assert!(resolved.is_none());
    }
}
