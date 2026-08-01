//! Resolving the resource a request names, and the account state that gates it.
//!
//! `aex_control_domain::decide` needs a **fully resolved** resource: a workspace
//! carries its organization, a key carries its workspace and organization, an
//! operation carries its organization. Those come from the row, never from the
//! path, because a caller who could name a foreign organization in the path
//! could otherwise authorize itself against it.
//!
//! Resolution therefore happens at precedence stage 3, before the handler, and
//! it is a port so the edge stays testable without a database.

use aex_control_domain::{
    AccountState, Action, Granted, Principal, Resource, ResourceClass, decide, requirement,
};
use aex_wire::routes::{PathBinding, RouteId};
use async_trait::async_trait;
use uuid::Uuid;

use crate::error::{EdgeError, from_denial};

/// The bound path parameters, owned so they can cross an `async` boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetPath(Vec<(String, String)>);

impl TargetPath {
    /// Copies a matched binding.
    #[must_use]
    pub fn from_binding(binding: &PathBinding<'_>) -> Self {
        Self(
            binding
                .as_slice()
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    /// The value bound to `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(bound, _)| bound == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Where the resource a route names is read from.
#[async_trait]
pub trait TargetResolver: Send + Sync {
    /// The fully resolved resource `route` names, or `None` when it does not
    /// exist.
    ///
    /// # Errors
    ///
    /// Returns [`EdgeError::AuthenticationUnavailable`] when the authority could
    /// not be reached; every other failure is the resolver's own.
    async fn resolve(
        &self,
        route: RouteId,
        path: &TargetPath,
    ) -> Result<Option<Resource>, EdgeError>;

    /// The **current** account state of an organization.
    ///
    /// There is deliberately no cached-positive path: an unreadable state
    /// rejects new admission, because serving a paused account and refusing a
    /// paying one are both worse than a retryable `503`.
    ///
    /// # Errors
    ///
    /// Returns [`EdgeError::AccountStateUnavailable`] when the state could not
    /// be established.
    async fn account_state(&self, organization_id: Uuid) -> Result<AccountState, EdgeError>;
}

/// Runs precedence stages 3 to 6 for one request.
///
/// The order is fixed and matches the wire contract: resolve, decide, then gate
/// on the account state. A `403` is therefore always decided before a `402`, and
/// a caller learns nothing about an organization it may not act in.
///
/// # Errors
///
/// Returns the first stage that refused, as an [`EdgeError`].
pub async fn admit_request(
    resolver: &dyn TargetResolver,
    principal: &Principal,
    action: Action,
    path: &TargetPath,
) -> Result<Granted, EdgeError> {
    let requirement = requirement(action);
    let resource = if requirement.resource_class == ResourceClass::None {
        Resource::None
    } else {
        resolver
            .resolve(action.route(), path)
            .await?
            .ok_or(EdgeError::NotFound)?
    };

    // Decide first. Reading the account state of an organization the caller may
    // not act in would be a side effect it is not entitled to cause, and the
    // wire contract's precedence puts `403` before `402` for the same reason.
    let granted = decide(principal, action, &resource).map_err(from_denial)?;

    // A pause-exempt route never reads the state at all, so an unreadable
    // finance row cannot take down a route that does not depend on it.
    if requirement.pause_exempt {
        return Ok(granted);
    }
    let Some(organization_id) = granted.organization_id else {
        return Ok(granted);
    };
    match resolver.account_state(organization_id).await? {
        AccountState::Active => Ok(granted),
        AccountState::PausedTopUpRequired => Err(EdgeError::AccountPaused),
        AccountState::Unavailable => Err(EdgeError::AccountStateUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::{TargetPath, TargetResolver, admit_request};
    use crate::error::EdgeError;
    use aex_control_domain::{
        AccountState, Action, ActorCredential, OrgMembership, OrgRole, Principal, Resource,
        ScopeSet,
    };
    use aex_wire::routes::RouteId;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    const ORGANIZATION: u128 = 0x0A;

    #[derive(Debug)]
    struct Stub {
        resource: Option<Resource>,
        state: Result<AccountState, ()>,
        resolutions: AtomicUsize,
        state_reads: AtomicUsize,
    }

    impl Stub {
        fn new(resource: Option<Resource>, state: Result<AccountState, ()>) -> Self {
            Self {
                resource,
                state,
                resolutions: AtomicUsize::new(0),
                state_reads: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl TargetResolver for Stub {
        async fn resolve(
            &self,
            _route: RouteId,
            _path: &TargetPath,
        ) -> Result<Option<Resource>, EdgeError> {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            Ok(self.resource)
        }

        async fn account_state(&self, _organization_id: Uuid) -> Result<AccountState, EdgeError> {
            self.state_reads.fetch_add(1, Ordering::SeqCst);
            self.state.map_err(|()| EdgeError::AccountStateUnavailable)
        }
    }

    fn owner() -> Principal {
        Principal::AccountActor {
            user_id: Uuid::from_u128(1),
            credential: ActorCredential::AccountToken(Uuid::from_u128(2)),
            memberships: vec![OrgMembership {
                organization_id: Uuid::from_u128(ORGANIZATION),
                membership_id: Uuid::from_u128(3),
                role: OrgRole::Owner,
            }],
            token_scopes: ScopeSet::ALL,
        }
    }

    fn action(route: RouteId) -> Action {
        Action::central(route).expect("a central route")
    }

    #[tokio::test]
    async fn an_org_scoped_route_resolves_its_resource_and_reads_the_state_once() {
        let stub = Stub::new(
            Some(Resource::Organization(Uuid::from_u128(ORGANIZATION))),
            Ok(AccountState::Active),
        );
        let granted = admit_request(
            &stub,
            &owner(),
            action(RouteId::MembershipsList),
            &TargetPath::default(),
        )
        .await
        .expect("an owner may list memberships");
        assert_eq!(granted.organization_id, Some(Uuid::from_u128(ORGANIZATION)));
        assert_eq!(stub.resolutions.load(Ordering::SeqCst), 1);
        assert_eq!(stub.state_reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_unreadable_account_state_rejects_admission_rather_than_falling_back() {
        let stub = Stub::new(
            Some(Resource::Organization(Uuid::from_u128(ORGANIZATION))),
            Err(()),
        );
        assert_eq!(
            admit_request(
                &stub,
                &owner(),
                action(RouteId::MembershipsList),
                &TargetPath::default()
            )
            .await,
            Err(EdgeError::AccountStateUnavailable)
        );
    }

    #[tokio::test]
    async fn a_pause_exempt_route_never_reads_the_account_state() {
        let stub = Stub::new(
            Some(Resource::Operation {
                operation_id: Uuid::from_u128(9),
                organization_id: Uuid::from_u128(ORGANIZATION),
            }),
            Err(()),
        );
        admit_request(
            &stub,
            &owner(),
            action(RouteId::CentralOperationGet),
            &TargetPath::default(),
        )
        .await
        .expect("an exempt route runs while the state is unreadable");
        assert_eq!(stub.state_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_absent_resource_is_not_found_before_any_state_is_read() {
        let stub = Stub::new(None, Ok(AccountState::Active));
        assert_eq!(
            admit_request(
                &stub,
                &owner(),
                action(RouteId::MembershipsList),
                &TargetPath::default()
            )
            .await,
            Err(EdgeError::NotFound)
        );
        assert_eq!(stub.state_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_actor_scoped_route_resolves_nothing_at_all() {
        let stub = Stub::new(None, Err(()));
        admit_request(
            &stub,
            &owner(),
            action(RouteId::OrganizationsList),
            &TargetPath::default(),
        )
        .await
        .expect("an actor-scoped route needs no resource");
        assert_eq!(stub.resolutions.load(Ordering::SeqCst), 0);
        assert_eq!(stub.state_reads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_paused_account_is_refused_on_a_non_exempt_route() {
        let stub = Stub::new(
            Some(Resource::Organization(Uuid::from_u128(ORGANIZATION))),
            Ok(AccountState::PausedTopUpRequired),
        );
        assert_eq!(
            admit_request(
                &stub,
                &owner(),
                action(RouteId::MembershipsList),
                &TargetPath::default()
            )
            .await,
            Err(EdgeError::AccountPaused)
        );
    }

    #[tokio::test]
    async fn a_member_of_another_organization_is_forbidden_before_the_state_is_read() {
        let stub = Stub::new(
            Some(Resource::Organization(Uuid::from_u128(0xFF))),
            Ok(AccountState::Active),
        );
        assert_eq!(
            admit_request(
                &stub,
                &owner(),
                action(RouteId::MembershipsList),
                &TargetPath::default()
            )
            .await,
            Err(EdgeError::Forbidden)
        );
    }
}
