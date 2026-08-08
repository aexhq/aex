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

use std::sync::Arc;

use aex_control_app::ports::{ControlStore, StoreError};
use aex_control_domain::{
    AccountState, Action, Granted, Principal, Resource, ResourceClass, WorkspaceStatus, decide,
    requirement,
};
use aex_wire::ids::{ApiKeyId, OperationId, OrganizationId, PrefixedId, WorkspaceId};
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

/// The resolver every control composition uses, over the coarse control port.
///
/// Each arm reads the **row** and takes the containing workspace and
/// organization from it. Nothing is taken from the path except the identifier
/// being looked up, which is why a caller who names a foreign organization in a
/// path cannot authorize itself against it: the decision runs against what the
/// row says the resource belongs to.
///
/// A deleted workspace is [`EdgeError::Gone`] rather than [`EdgeError::NotFound`].
/// The two are different instructions — one says "you had the wrong name", the
/// other says "this existed and does not any more" — and only the second tells
/// a client to stop retrying.
pub struct ControlStoreTargets {
    store: Arc<dyn ControlStore>,
}

impl std::fmt::Debug for ControlStoreTargets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ControlStoreTargets").finish()
    }
}

impl ControlStoreTargets {
    /// Builds the resolver over one control authority.
    #[must_use]
    pub fn new(store: Arc<dyn ControlStore>) -> Self {
        Self { store }
    }

    /// The identifier a path binds under `name`, in its wire form.
    ///
    /// An identifier that does not parse names nothing, so it is
    /// [`EdgeError::NotFound`] rather than a `400`: telling an unauthorized
    /// caller apart "that is not an identifier" from "that identifier is not
    /// yours" is information it has not earned, and the route's own declared
    /// codes do not include a validation failure on a path parameter.
    fn bound<T: PrefixedId>(path: &TargetPath, name: &'static str) -> Result<Uuid, EdgeError> {
        let text = path.get(name).ok_or(EdgeError::Internal(
            "a route bound no path parameter its resource class requires",
        ))?;
        let parsed = T::parse(text).map_err(|_| EdgeError::NotFound)?;
        Ok(Uuid::from_bytes(*parsed.uuid7().as_bytes()))
    }

    /// A store failure at resolution time is never a `404`.
    ///
    /// `NotFound` from the store means the read ran and matched nothing, which
    /// the caller of this function turns into `404` itself. Everything else is
    /// an authority that did not answer, and answering `404` for that would
    /// tell a caller its resource is gone because a database was busy.
    const fn unreadable(error: &StoreError) -> EdgeError {
        match error {
            StoreError::NotFound => EdgeError::NotFound,
            _ => EdgeError::AuthenticationUnavailable,
        }
    }
}

#[async_trait]
impl TargetResolver for ControlStoreTargets {
    async fn resolve(
        &self,
        route: RouteId,
        path: &TargetPath,
    ) -> Result<Option<Resource>, EdgeError> {
        let action = Action::central(route).map_err(|_| {
            EdgeError::Internal("a regional route reached the central target resolver")
        })?;
        match requirement(action).resource_class {
            // `admit_request` never calls this arm; it short-circuits an
            // actor-scoped route before the resolver is consulted. Answering
            // `None` here would render as `404` for a route that names nothing.
            ResourceClass::None => Ok(Some(Resource::None)),
            ResourceClass::Organization => {
                let id = Self::bound::<OrganizationId>(path, "organizationId")?;
                let found = self
                    .store
                    .get_organization(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?;
                Ok(found.map(|organization| Resource::Organization(organization.id)))
            }
            ResourceClass::Workspace => {
                let id = Self::bound::<WorkspaceId>(path, "workspaceId")?;
                let Some(workspace) = self
                    .store
                    .get_workspace(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?
                else {
                    return Ok(None);
                };
                if workspace.status == WorkspaceStatus::Deleted {
                    return Err(EdgeError::Gone);
                }
                Ok(Some(Resource::Workspace {
                    workspace_id: workspace.id,
                    organization_id: workspace.organization_id,
                }))
            }
            ResourceClass::ApiKey => {
                let id = Self::bound::<ApiKeyId>(path, "apiKeyId")?;
                let found = self
                    .store
                    .get_api_key(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?;
                Ok(found.map(|key| Resource::ApiKey {
                    key_id: key.id,
                    workspace_id: key.workspace_id,
                    organization_id: key.organization_id,
                }))
            }
            ResourceClass::Operation => {
                let id = Self::bound::<OperationId>(path, "operationId")?;
                let found = self
                    .store
                    .get_operation(id)
                    .await
                    .map_err(|error| Self::unreadable(&error))?;
                Ok(found.map(|operation| Resource::Operation {
                    operation_id: operation.id,
                    organization_id: operation.organization_id,
                }))
            }
        }
    }

    async fn account_state(&self, organization_id: Uuid) -> Result<AccountState, EdgeError> {
        // A failure is never softened into a state. The port already answers
        // `Unavailable` for an organization whose finance row is absent, so the
        // only thing left to map here is a store that did not answer at all —
        // and that is the same refusal, not a fallback to `Active`.
        self.store
            .account_state(organization_id)
            .await
            .map_err(|_| EdgeError::AccountStateUnavailable)
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
    async fn workspace_create_defers_its_body_scoped_organization_gate_to_the_handler() {
        let stub = Stub::new(None, Err(()));
        let granted = admit_request(
            &stub,
            &owner(),
            action(RouteId::WorkspaceCreate),
            &TargetPath::default(),
        )
        .await
        .expect("the edge admits the actor and scope before the body target is decoded");
        assert_eq!(granted.organization_id, None);
        assert_eq!(granted.role, None);
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

#[cfg(test)]
mod store_targets {
    //! The store-backed resolver, over a control authority that answers only
    //! the four reads a resolution needs.

    use super::{ControlStoreTargets, TargetPath, TargetResolver as _};
    use crate::error::EdgeError;
    use aex_control_app::ports::{
        AcceptInvitationsTx, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx,
        ClaimDueOperations, ClaimOutbox, CompleteWorkspaceDeletionTx, ControlStore, CreateApiKeyTx,
        CreateInvitationTx, CreateOrganizationTx, FinishWorkspaceProvisionTx, GcExpired, GcReport,
        ListApiKeys, ListOperations, ListOrganizations, ListWorkspaces, Page, PageRequest,
        RevokeApiKeyTx, StoreError, TxOutcome,
    };
    use aex_control_domain::{
        AccountState, ApiKey, Fence, IntentHash, Invitation, Membership, Operation, OperationKind,
        OperationStatus, OperationVisibility, Organization, OrganizationStatus, OutboxMessage,
        Resource, Revision, ScopeSet, Slug, Workspace, WorkspaceStatus,
    };
    use aex_wire::ids::{
        ApiKeyId, OperationId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId,
    };
    use aex_wire::routes::{Plane, RouteId, match_route, route};
    use async_trait::async_trait;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use uuid::Uuid;

    const ORGANIZATION: u8 = 0x11;
    const WORKSPACE: u8 = 0x22;
    const API_KEY: u8 = 0x33;
    const OPERATION: u8 = 0x44;

    fn uuid7(tag: u8) -> Uuid7 {
        Uuid7::compose(1_767_225_600_000, [tag; 10])
    }

    fn raw(tag: u8) -> Uuid {
        Uuid::from_bytes(*uuid7(tag).as_bytes())
    }

    fn at() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH
    }

    /// Every read a resolution never performs. Reaching one is a defect in the
    /// resolver rather than a gap in this double: a resolver that listed a
    /// collection to answer "which organization owns this row?" would be doing
    /// far more work than the question needs, on the request path.
    const UNRESOLVED: &str = "a target resolution reads one row and nothing else";

    /// What the control authority answers, per read.
    #[derive(Debug, Default)]
    struct Answers {
        organization: Option<Organization>,
        workspace: Option<Workspace>,
        api_key: Option<ApiKey>,
        operation: Option<Operation>,
        account_state: Option<AccountState>,
        store_is_down: bool,
    }

    fn organization() -> Organization {
        Organization {
            id: raw(ORGANIZATION),
            name: "Acme".to_owned(),
            slug: Slug::parse("acme").expect("a valid slug"),
            status: OrganizationStatus::Active,
            revision: Revision::INITIAL,
            created_at: at(),
            updated_at: at(),
            created_by_user_id: raw(0x55),
        }
    }

    fn workspace(status: WorkspaceStatus) -> Workspace {
        Workspace {
            id: raw(WORKSPACE),
            organization_id: raw(ORGANIZATION),
            name: "Prod".to_owned(),
            slug: Slug::parse("prod").expect("a valid slug"),
            region: aex_wire::types::Region::EuWest1,
            status,
            provision_operation_id: raw(0x66),
            provision_fence: Fence::FIRST,
            deletion_operation_id: None,
            deletion_fence: None,
            revision: Revision::INITIAL,
            created_at: at(),
            updated_at: at(),
            activated_at: None,
            deleted_at: None,
            created_by_user_id: raw(0x55),
        }
    }

    fn api_key() -> ApiKey {
        ApiKey {
            id: raw(API_KEY),
            workspace_id: raw(WORKSPACE),
            organization_id: raw(ORGANIZATION),
            name: "ci".to_owned(),
            scopes: ScopeSet::EMPTY,
            region: aex_wire::types::Region::EuWest1,
            pepper_version: 1,
            created_at: at(),
            revoked_at: None,
            revision: Revision::INITIAL,
            created_by_user_id: raw(0x55),
        }
    }

    fn operation() -> Operation {
        Operation {
            id: raw(OPERATION),
            kind: OperationKind::WorkspaceDelete,
            visibility: OperationVisibility::Public,
            organization_id: raw(ORGANIZATION),
            workspace_id: Some(raw(WORKSPACE)),
            principal_id: raw(0x55),
            scopes: ScopeSet::EMPTY,
            status: OperationStatus::Queued,
            intent_hash: IntentHash::from_bytes([0; 32]),
            fence: Fence::FIRST,
            attempt: 0,
            lease: None,
            created_at: at(),
            started_at: None,
            updated_at: at(),
            terminal_at: None,
            due_at: None,
        }
    }

    #[derive(Debug)]
    struct Reads(Answers);

    #[async_trait]
    impl ControlStore for Reads {
        async fn create_organization(
            &self,
            _command: &CreateOrganizationTx,
        ) -> Result<TxOutcome<Organization>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn list_organizations(
            &self,
            _query: &ListOrganizations,
        ) -> Result<Page<Organization>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn get_organization(&self, _id: Uuid) -> Result<Option<Organization>, StoreError> {
            if self.0.store_is_down {
                return Err(StoreError::Unavailable);
            }
            Ok(self.0.organization.clone())
        }

        async fn list_memberships(
            &self,
            _organization_id: Uuid,
            _page: &PageRequest,
        ) -> Result<Page<Membership>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn create_invitation(
            &self,
            _command: &CreateInvitationTx,
        ) -> Result<TxOutcome<Invitation>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn accept_invitations_for_email(
            &self,
            _command: &AcceptInvitationsTx,
        ) -> Result<TxOutcome<Vec<Membership>>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn begin_workspace_provision(
            &self,
            _command: &BeginWorkspaceProvisionTx,
        ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn finish_workspace_provision(
            &self,
            _command: &FinishWorkspaceProvisionTx,
        ) -> Result<TxOutcome<Workspace>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn list_workspaces(
            &self,
            _query: &ListWorkspaces,
        ) -> Result<Page<Workspace>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn get_workspace(&self, _id: Uuid) -> Result<Option<Workspace>, StoreError> {
            Ok(self.0.workspace.clone())
        }

        async fn begin_workspace_deletion(
            &self,
            _command: &BeginWorkspaceDeletionTx,
        ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn complete_workspace_deletion(
            &self,
            _command: &CompleteWorkspaceDeletionTx,
        ) -> Result<TxOutcome<Workspace>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn create_api_key(
            &self,
            _command: &CreateApiKeyTx,
        ) -> Result<TxOutcome<ApiKey>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn get_api_key(&self, _id: Uuid) -> Result<Option<ApiKey>, StoreError> {
            Ok(self.0.api_key.clone())
        }

        async fn list_api_keys(&self, _query: &ListApiKeys) -> Result<Page<ApiKey>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn revoke_api_key(
            &self,
            _command: &RevokeApiKeyTx,
        ) -> Result<TxOutcome<()>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn get_operation(&self, _id: Uuid) -> Result<Option<Operation>, StoreError> {
            Ok(self.0.operation.clone())
        }

        async fn list_operations(
            &self,
            _query: &ListOperations,
        ) -> Result<Page<Operation>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn claim_due_operations(
            &self,
            _command: &ClaimDueOperations,
        ) -> Result<Vec<Operation>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn claim_outbox(
            &self,
            _command: &ClaimOutbox,
        ) -> Result<Vec<OutboxMessage>, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn enqueue_outbox(&self, _message: &OutboxMessage) -> Result<(), StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn mark_outbox_dispatched(
            &self,
            _id: Uuid,
            _now: OffsetDateTime,
        ) -> Result<(), StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn release_outbox(
            &self,
            _id: Uuid,
            _available_at: OffsetDateTime,
            _error: &str,
        ) -> Result<(), StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn gc_expired(&self, _command: &GcExpired) -> Result<GcReport, StoreError> {
            unreachable!("{UNRESOLVED}")
        }

        async fn account_state(&self, _organization_id: Uuid) -> Result<AccountState, StoreError> {
            if self.0.store_is_down {
                return Err(StoreError::Unavailable);
            }
            self.0.account_state.ok_or(StoreError::Unavailable)
        }
    }

    fn resolver(answers: Answers) -> ControlStoreTargets {
        ControlStoreTargets::new(Arc::new(Reads(answers)))
    }

    /// The binding a real request against `id` would carry.
    ///
    /// Built by running the concrete path back through the generated matcher
    /// rather than by assembling a binding by hand, so a template whose
    /// parameter is renamed fails here rather than silently binding nothing.
    fn path(id: RouteId, parameter: &str) -> TargetPath {
        let descriptor = route(id);
        let concrete = descriptor
            .template
            .replace("{organizationId}", parameter)
            .replace("{workspaceId}", parameter)
            .replace("{apiKeyId}", parameter)
            .replace("{operationId}", parameter);
        let (matched, binding) = match_route(Plane::Central, descriptor.method, &concrete)
            .expect("a concrete path matches its own template");
        assert_eq!(matched, id, "the matcher resolved another route");
        TargetPath::from_binding(&binding)
    }

    fn organization_path() -> TargetPath {
        path(
            RouteId::MembershipsList,
            OrganizationId::from_uuid7(uuid7(ORGANIZATION))
                .encode()
                .as_str(),
        )
    }

    fn workspace_path() -> TargetPath {
        path(
            RouteId::WorkspaceGet,
            WorkspaceId::from_uuid7(uuid7(WORKSPACE)).encode().as_str(),
        )
    }

    #[tokio::test]
    async fn every_resource_class_takes_its_containment_from_the_row() {
        let resolver = resolver(Answers {
            organization: Some(organization()),
            workspace: Some(workspace(WorkspaceStatus::Active)),
            api_key: Some(api_key()),
            operation: Some(operation()),
            ..Answers::default()
        });

        assert_eq!(
            resolver
                .resolve(RouteId::MembershipsList, &organization_path())
                .await
                .expect("the organization resolves"),
            Some(Resource::Organization(raw(ORGANIZATION)))
        );

        assert_eq!(
            resolver
                .resolve(RouteId::WorkspaceGet, &workspace_path())
                .await
                .expect("the workspace resolves"),
            Some(Resource::Workspace {
                workspace_id: raw(WORKSPACE),
                organization_id: raw(ORGANIZATION),
            })
        );

        assert_eq!(
            resolver
                .resolve(
                    RouteId::ApiKeyRevoke,
                    &path(
                        RouteId::ApiKeyRevoke,
                        ApiKeyId::from_uuid7(uuid7(API_KEY)).encode().as_str(),
                    ),
                )
                .await
                .expect("the key resolves"),
            Some(Resource::ApiKey {
                key_id: raw(API_KEY),
                workspace_id: raw(WORKSPACE),
                organization_id: raw(ORGANIZATION),
            })
        );

        assert_eq!(
            resolver
                .resolve(
                    RouteId::CentralOperationGet,
                    &path(
                        RouteId::CentralOperationGet,
                        OperationId::from_uuid7(uuid7(OPERATION)).encode().as_str(),
                    ),
                )
                .await
                .expect("the operation resolves"),
            Some(Resource::Operation {
                operation_id: raw(OPERATION),
                organization_id: raw(ORGANIZATION),
            })
        );
    }

    #[tokio::test]
    async fn an_identifier_of_another_kind_names_nothing() {
        // A workspace identifier in an organization slot is not a validation
        // failure a caller is told about: it names no organization, and saying
        // more would tell an unauthorized caller "malformed" and "not yours"
        // apart.
        let resolver = resolver(Answers {
            organization: Some(organization()),
            ..Answers::default()
        });
        assert_eq!(
            resolver
                .resolve(
                    RouteId::MembershipsList,
                    &path(
                        RouteId::MembershipsList,
                        WorkspaceId::from_uuid7(uuid7(WORKSPACE)).encode().as_str(),
                    ),
                )
                .await,
            Err(EdgeError::NotFound)
        );
    }

    #[tokio::test]
    async fn a_deleted_workspace_is_gone_rather_than_absent() {
        let resolver = resolver(Answers {
            workspace: Some(workspace(WorkspaceStatus::Deleted)),
            ..Answers::default()
        });
        assert_eq!(
            resolver
                .resolve(RouteId::WorkspaceGet, &workspace_path())
                .await,
            Err(EdgeError::Gone),
            "a tombstone tells a client to stop retrying; a 404 does not"
        );
    }

    #[tokio::test]
    async fn an_absent_row_is_absent_rather_than_fabricated() {
        let resolver = resolver(Answers::default());
        assert_eq!(
            resolver
                .resolve(RouteId::WorkspaceGet, &workspace_path())
                .await
                .expect("the read answers"),
            None
        );
    }

    #[tokio::test]
    async fn an_unreadable_authority_never_renders_as_a_missing_resource() {
        let resolver = resolver(Answers {
            store_is_down: true,
            ..Answers::default()
        });
        assert_eq!(
            resolver
                .resolve(RouteId::MembershipsList, &organization_path())
                .await,
            Err(EdgeError::AuthenticationUnavailable),
            "a busy database must not tell a caller its organization is gone"
        );
        assert_eq!(
            resolver.account_state(raw(ORGANIZATION)).await,
            Err(EdgeError::AccountStateUnavailable)
        );
    }

    #[tokio::test]
    async fn the_account_state_is_carried_through_unchanged_in_all_three_directions() {
        for state in [
            AccountState::Active,
            AccountState::PausedTopUpRequired,
            AccountState::Unavailable,
        ] {
            let resolver = resolver(Answers {
                account_state: Some(state),
                ..Answers::default()
            });
            assert_eq!(
                resolver.account_state(raw(ORGANIZATION)).await,
                Ok(state),
                "the edge decides what {state:?} means; the resolver never softens it"
            );
        }
    }
}
