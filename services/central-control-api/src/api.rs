//! Generated central-control handlers over the application ports.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_central_http::cursor::{PageBinding, next_cursor, page_request};
use aex_control_app::ports::{
    AcceptInvitationsTx, AccountProjection, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx,
    ControlStore, ControlViewStore, CreateApiKeyTx, CreateInvitationTx, CreateOrganizationTx,
    IdempotencyRecordKey, ListApiKeys, ListOperations, ListOrganizations, ListWorkspaces,
    PageRequest, RegionalControlPort, RevokeApiKeyTx, StoreError,
};
use aex_control_app::{
    AcceptInvitations, CancelOperation, ControlError, CreateApiKey, CreateInvitation,
    CreateOrganization, CreateWorkspace, DeleteWorkspace, RevokeApiKey,
};
use aex_control_domain::{
    AccountProjectionError, AccountState, ActorKind, ApiKey as DomainApiKey, AuditEvent,
    AuditOutcome, CursorSecret, account_operational_state,
    IdempotencyKeyKind, IntentHash, Invitation as DomainInvitation, MAX_ACCEPTABLE_INVITATIONS,
    InvitationStatus as DomainInvitationStatus, MembershipStatus as DomainMembershipStatus,
    OperationStatus as DomainOperationStatus, OrgRole, Organization as DomainOrganization,
    OutboxMessage, PrincipalKindTag, ResourceKind, ScopeKind, ScopeSet as DomainScopeSet, Slug,
    Topic, WorkspaceStatus as DomainWorkspaceStatus,
};
use aex_identity_app::ports::{Clock, IdFactory, PepperKeystore, PepperPurpose};
use aex_identity_domain::credential::{
    CredentialKind, RegionCode, SecretRng, WorkspacePin, mint, verifier,
};
use aex_wire::cursor::Cursor;
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{
    ApiKeyId, InvitationId, MembershipId, OperationId, OrganizationId, PrefixedId, UserId, Uuid7,
    WorkspaceId,
};
use aex_wire::models::{
    AccountOperationalState, ApiKey,
    ApiKeyCreateRequest, ApiKeyPage, ApiKeysListQuery, CentralOperationsListQuery,
    DashboardBootstrap, EmptyRequest, Invitation, InvitationAcceptResult, InvitationCreateRequest,
    InvitationRole, InvitationStatus, Membership, MembershipPage, MembershipStatus,
    MembershipsListQuery, NewApiKey, Operation, OperationKind, OperationPage, OperationResult,
    OperationStatus, OperationalStateSource, Organization, OrganizationAccount, OrganizationCreateRequest,
    OrganizationPage, OrganizationRole, OrganizationsListQuery, Workspace, WorkspaceCreateRequest,
    WorkspaceDeleteRequest, WorkspaceOperationalState, WorkspacePage, WorkspaceStatus,
    WorkspaceTombstone, WorkspacesListQuery,
};
use aex_wire::routes::{Plane, match_route, route};
use aex_wire::server::{
    Accepted, ApiKeysApi, BootstrapApi, CentralOperationsApi, Created, NoContent, OrganizationsApi,
    RequestContext, WorkspacesApi,
};
use aex_wire::types::{ETag, HttpsUrl, Region, Timestamp};
use serde::Serialize;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// The complete store surface used by the public control boundary.
pub trait Store: ControlStore + ControlViewStore {}

impl<T: ControlStore + ControlViewStore> Store for T {}

struct AuditSpec<'a> {
    organization_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    action: &'a str,
    resource_kind: ResourceKind,
    resource_id: Option<Uuid>,
    operation_id: Option<Uuid>,
    detail: serde_json::Value,
}

/// The generated central-control implementation.
pub struct ControlService {
    store: Arc<dyn Store>,
    peppers: Arc<dyn PepperKeystore>,
    regional: Arc<dyn RegionalControlPort>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdFactory>,
    rng: Arc<dyn SecretRng>,
    cursor_secret: Arc<CursorSecret>,
    region: Region,
    api_urls: BTreeMap<Region, HttpsUrl>,
}

impl ControlService {
    /// Composes the handler over its explicit authorities and ambient primitives.
    #[allow(
        clippy::too_many_arguments,
        reason = "the composition root names every authority"
    )]
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        peppers: Arc<dyn PepperKeystore>,
        regional: Arc<dyn RegionalControlPort>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdFactory>,
        rng: Arc<dyn SecretRng>,
        cursor_secret: Arc<CursorSecret>,
        region: Region,
        api_urls: BTreeMap<Region, HttpsUrl>,
    ) -> Self {
        Self {
            store,
            peppers,
            regional,
            clock,
            ids,
            rng,
            cursor_secret,
            region,
            api_urls,
        }
    }

    fn now(&self) -> OffsetDateTime {
        self.clock.now()
    }

    fn now_ms(&self) -> i64 {
        millis(self.now())
    }

    fn user(cx: &RequestContext) -> WireResult<Uuid> {
        match cx.principal {
            PrincipalScope::Account { user, .. } => Ok(raw(user)),
            PrincipalScope::WorkspaceKey { .. } => Err(WireError::new(ErrorCode::Forbidden)),
        }
    }

    fn admit_admin_role(role: OrgRole) -> WireResult<()> {
        if role.at_least(OrgRole::Admin) {
            Ok(())
        } else {
            Err(WireError::new(ErrorCode::Forbidden))
        }
    }

    /// Whether this handler owns the account-state gate for `id`.
    ///
    /// The split is not a preference. `admit_request` runs the gate itself for
    /// every route that is not `pause_exempt` **and resolves its organization
    /// from the path**. A route whose organization arrives in a query parameter
    /// or a body resolves [`ResourceClass::None`], which the edge treats as
    /// exempt, so its handler owns the gate instead — `aex_control_domain`'s
    /// `non_path_targets_defer_their_organization_gate_to_the_handler` pins
    /// exactly that.
    ///
    /// Running the gate on the other side of the split is not a harmless second
    /// read. `account_paused` and `account_state_unavailable` are the edge's
    /// codes, and a route the edge already gates has no reason to declare them
    /// — `memberships_list` does not — so the duplicate refusal reaches
    /// [`aex_wire::dispatch::declared`] undeclared and is rendered
    /// `500 internal_error` instead of the `402` the edge had already produced.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InternalError`] when a regional route reached a
    /// central handler, which means the router is composed wrong.
    fn handler_owns_account_gate(id: aex_wire::routes::RouteId) -> WireResult<bool> {
        let action = aex_control_domain::Action::central(id)
            .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        Ok(aex_control_domain::requirement(action).resource_class
            == aex_control_domain::ResourceClass::None)
    }

    async fn require_admin(&self, cx: &RequestContext, organization_id: Uuid) -> WireResult<()> {
        let role = self.organization_access(cx, organization_id).await?;
        Self::admit_admin_role(role)
    }

    /// Resolves the caller's role, gating the account state where this handler
    /// owns that gate.
    ///
    /// Whether it does is derived from the route rather than passed in, so a
    /// call site cannot re-gate a route the edge already gated.
    async fn organization_access(
        &self,
        cx: &RequestContext,
        organization_id: Uuid,
    ) -> WireResult<OrgRole> {
        let user_id = Self::user(cx)?;
        let view = self
            .store
            .get_organization_view(organization_id, user_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::Forbidden))?;
        if Self::handler_owns_account_gate(cx.route)? {
            match self
                .store
                .account_profile(organization_id)
                .await
                .map_err(store_error)?
                .map(|projection| projection.state())
            {
                Some(AccountState::Active) => {}
                Some(AccountState::PausedTopUpRequired) => {
                    return Err(WireError::new(ErrorCode::AccountPaused));
                }
                Some(AccountState::Unavailable) | None => {
                    return Err(WireError::new(ErrorCode::AccountStateUnavailable));
                }
            }
        }
        Ok(view.caller_role)
    }

    fn page_binding(
        &self,
        cx: &RequestContext,
        principal_id: Uuid,
        scope_id: Uuid,
        filters: &[(&str, &str)],
    ) -> PageBinding {
        PageBinding {
            endpoint: route(cx.route).template,
            principal_id,
            scope_id,
            region: self.region,
            filter_hash: PageBinding::filter_hash(filters),
            snapshot_ms: self.now_ms(),
        }
    }

    fn page(
        &self,
        binding: &PageBinding,
        cursor: Option<&Cursor>,
        limit: Option<u32>,
    ) -> WireResult<PageRequest> {
        page_request(
            &self.cursor_secret,
            binding,
            cursor.map(Cursor::as_str),
            limit,
            self.now_ms(),
        )
        .map_err(aex_central_http::error::EdgeError::into_wire)
    }

    fn cursor(
        &self,
        binding: &PageBinding,
        next: Option<(i64, Uuid)>,
    ) -> WireResult<Option<Cursor>> {
        next_cursor(&self.cursor_secret, binding, next, self.now_ms())
            .map(|raw| Cursor::parse(&raw).map_err(|_| WireError::new(ErrorCode::InternalError)))
            .transpose()
    }

    fn idempotency<T: Serialize>(
        &self,
        cx: &RequestContext,
        body: &T,
        scope_kind: ScopeKind,
        scope_id: Uuid,
        concrete_path: &str,
    ) -> WireResult<IdempotencyRecordKey> {
        let descriptor = route(cx.route);
        let (_, path) = match_route(Plane::Central, descriptor.method, concrete_path)
            .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
        let canonical =
            aex_wire::to_jcs_bytes(body).map_err(|_| WireError::new(ErrorCode::InvalidRequest))?;
        let intent = aex_wire::intent_digest(cx.route, &path, Some(&canonical));
        let (key_kind, key_value) = match descriptor.idempotency {
            aex_wire::idempotency::IdempotencyKind::IdempotencyKey => (
                IdempotencyKeyKind::IdempotencyKey,
                cx.idempotency_key
                    .as_ref()
                    .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest))?
                    .as_str()
                    .to_owned(),
            ),
            aex_wire::idempotency::IdempotencyKind::OperationId => (
                IdempotencyKeyKind::OperationId,
                cx.operation_id
                    .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest))?
                    .encode()
                    .as_str()
                    .to_owned(),
            ),
            aex_wire::idempotency::IdempotencyKind::None => {
                return Err(WireError::new(ErrorCode::InternalError));
            }
        };
        Ok(IdempotencyRecordKey {
            id: self.ids.next(),
            key_kind,
            key_value,
            principal_kind: PrincipalKindTag::AccountActor,
            principal_id: Self::user(cx)?,
            scope_kind,
            scope_id,
            method: descriptor.method,
            route: descriptor.template.to_owned(),
            intent_hash: IntentHash::from_bytes(*intent.as_bytes()),
            expires_at: self.now() + Duration::days(1),
        })
    }

    fn audit(&self, cx: &RequestContext, spec: AuditSpec<'_>) -> WireResult<AuditEvent> {
        if !aex_control_domain::audit::detail_is_permitted(&spec.detail) {
            return Err(WireError::new(ErrorCode::InternalError));
        }
        Ok(AuditEvent {
            id: self.ids.next(),
            organization_id: spec.organization_id,
            workspace_id: spec.workspace_id,
            actor_kind: ActorKind::User,
            actor_id: Some(Self::user(cx)?),
            action: spec.action.to_owned(),
            resource_kind: spec.resource_kind,
            resource_id: spec.resource_id,
            outcome: AuditOutcome::Allowed,
            request_id: cx.request_id.as_str().to_owned(),
            operation_id: spec.operation_id,
            detail: spec.detail,
            occurred_at: self.now(),
        })
    }

    fn outbox(
        &self,
        topic: Topic,
        dedupe_key: String,
        organization_id: Uuid,
        payload: serde_json::Value,
    ) -> OutboxMessage {
        let now = self.now();
        OutboxMessage {
            id: self.ids.next(),
            topic,
            dedupe_key,
            group_key: organization_id.to_string(),
            payload,
            attempts: 0,
            available_at: now,
            claimed_by: None,
            claimed_until: None,
            dispatched_at: None,
            last_error: None,
            created_at: now,
        }
    }

    async fn workspace_view(&self, id: Uuid) -> WireResult<aex_control_app::ports::WorkspaceView> {
        self.store
            .get_workspace_view(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))
    }

    fn workspace(&self, view: &aex_control_app::ports::WorkspaceView) -> WireResult<Workspace> {
        workspace_wire(view, &self.api_urls)
    }
}

impl OrganizationsApi for ControlService {
    async fn organizations_list(
        &self,
        cx: &RequestContext,
        query: OrganizationsListQuery,
    ) -> WireResult<OrganizationPage> {
        let user_id = Self::user(cx)?;
        let binding = self.page_binding(cx, user_id, user_id, &[]);
        let page = self.page(&binding, query.cursor.as_ref(), query.limit)?;
        let result = self
            .store
            .list_organization_views(&ListOrganizations { user_id, page })
            .await
            .map_err(store_error)?;
        Ok(OrganizationPage {
            items: result
                .items
                .iter()
                .map(organization_wire)
                .collect::<WireResult<_>>()?,
            next_cursor: self.cursor(&binding, result.next)?,
        })
    }

    async fn organization_create(
        &self,
        cx: &RequestContext,
        body: OrganizationCreateRequest,
    ) -> WireResult<Created<Organization>> {
        if !DomainOrganization::name_is_valid(&body.name) {
            return Err(WireError::new(ErrorCode::InvalidRequest));
        }
        let user_id = Self::user(cx)?;
        let organization_id = self.ids.next();
        let slug = slug(&body.name)?;
        let command = CreateOrganizationTx {
            preassigned_id: organization_id,
            preassigned_membership_id: self.ids.next(),
            name: body.name.clone(),
            slug: slug.clone(),
            created_by_user_id: user_id,
            idempotency: self.idempotency(
                cx,
                &body,
                ScopeKind::Organization,
                user_id,
                "/api/organizations",
            )?,
            audit: self.audit(
                cx,
                AuditSpec {
                    organization_id: Some(organization_id),
                    workspace_id: None,
                    action: "organization.create",
                    resource_kind: ResourceKind::Organization,
                    resource_id: Some(organization_id),
                    operation_id: None,
                    detail: serde_json::json!({ "slug": slug.as_str() }),
                },
            )?,
            now: self.now(),
        };
        let organization = CreateOrganization::run(self.store.as_ref(), &command)
            .await
            .map_err(|error| control_error(cx, error))?;
        Ok(Created(organization_wire(
            &aex_control_app::ports::OrganizationView {
                organization,
                caller_role: OrgRole::Owner,
            },
        )?))
    }

    async fn organization_get(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
    ) -> WireResult<Organization> {
        self.store
            .get_organization_view(raw(organization_id), Self::user(cx)?)
            .await
            .map_err(store_error)?
            .map(|view| organization_wire(&view))
            .transpose()?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))
    }

    async fn memberships_list(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        query: MembershipsListQuery,
    ) -> WireResult<MembershipPage> {
        let organization_id = raw(organization_id);
        self.organization_access(cx, organization_id).await?;
        let user_id = Self::user(cx)?;
        let binding = self.page_binding(cx, user_id, organization_id, &[]);
        let page = self.page(&binding, query.cursor.as_ref(), query.limit)?;
        let result = self
            .store
            .list_membership_views(organization_id, &page)
            .await
            .map_err(store_error)?;
        Ok(MembershipPage {
            items: result
                .items
                .iter()
                .map(membership_wire)
                .collect::<WireResult<_>>()?,
            next_cursor: self.cursor(&binding, result.next)?,
        })
    }

    async fn invitation_create(
        &self,
        cx: &RequestContext,
        organization_id: OrganizationId,
        body: InvitationCreateRequest,
    ) -> WireResult<Created<Invitation>> {
        let organization_id = raw(organization_id);
        self.organization_access(cx, organization_id).await?;
        let invitation_id = self.ids.next();
        let role = match body.role {
            InvitationRole::Admin => OrgRole::Admin,
            InvitationRole::Member => OrgRole::Member,
        };
        let email = body.email.trim().to_lowercase();
        if email.len() < 3 || email.len() > 254 || !email.contains('@') {
            return Err(WireError::new(ErrorCode::InvalidRequest));
        }
        let path = format!(
            "/api/organizations/{}/invitations",
            wire::<OrganizationId>(organization_id)?
        );
        let command = CreateInvitationTx {
            preassigned_id: invitation_id,
            organization_id,
            email: email.clone(),
            role,
            invited_by_user_id: Self::user(cx)?,
            expires_at: self.now() + Duration::days(14),
            idempotency: self.idempotency(
                cx,
                &body,
                ScopeKind::Organization,
                organization_id,
                &path,
            )?,
            outbox: self.outbox(
                Topic::InvitationEmailRequested,
                invitation_id.to_string(),
                organization_id,
                serde_json::json!({
                    "invitationId": invitation_id,
                    "organizationId": organization_id,
                    "email": email,
                    "role": role.as_str(),
                }),
            ),
            audit: self.audit(
                cx,
                AuditSpec {
                    organization_id: Some(organization_id),
                    workspace_id: None,
                    action: "invitation.create",
                    resource_kind: ResourceKind::Invitation,
                    resource_id: Some(invitation_id),
                    operation_id: None,
                    detail: serde_json::json!({ "role": role.as_str() }),
                },
            )?,
            now: self.now(),
        };
        let invitation = CreateInvitation::run(self.store.as_ref(), &command)
            .await
            .map_err(|error| control_error(cx, error))?;
        Ok(Created(invitation_wire(&invitation)?))
    }

    /// Redeems every pending invitation addressed to the caller's own verified
    /// email.
    ///
    /// # What identifies the invitation
    ///
    /// Nothing the caller sends. `control.invitation` has no token column
    /// on purpose — acceptance is a verified-email match — so the body is
    /// empty and the selection is the caller's own address. That also makes
    /// the route naturally idempotent without a replay identity: the
    /// selection reads `status = 'pending'`, so a second call finds nothing
    /// and answers an empty list rather than a conflict.
    ///
    /// # Why it resolves no organization
    ///
    /// The person redeeming is not a member of the inviting organization yet,
    /// so there is no membership to authorize against and no role floor that
    /// would not refuse every legitimate request. Tenant isolation is kept by
    /// the selection instead: the transaction reads only rows whose `email`
    /// equals this caller's verified address and takes the organization from
    /// the row.
    ///
    /// # The three refusals
    ///
    /// An unverified address is `403 forbidden` here rather than an empty
    /// success. `AcceptInvitations::run` answers an empty vector for it, which
    /// is the right shape for the use case and the wrong answer for a caller:
    /// "you have no invitations" and "your address is not verified, so none of
    /// them can be redeemed" are different instructions. Expired, revoked and
    /// already-accepted invitations are not refusals at all — they are simply
    /// not selected, which is what makes a retry safe.
    async fn invitation_accept(
        &self,
        cx: &RequestContext,
        _body: EmptyRequest,
    ) -> WireResult<InvitationAcceptResult> {
        let user_id = Self::user(cx)?;
        let identity = self
            .store
            .user_identity(user_id)
            .await
            .map_err(identity_read_error)?
            .ok_or_else(|| WireError::new(ErrorCode::Forbidden))?;
        if !identity.email_verified {
            return Err(WireError::new(ErrorCode::Forbidden)
                .with_message("an unverified email address cannot accept an invitation"));
        }
        let command = AcceptInvitationsTx {
            user_id,
            email: identity.email.clone(),
            email_verified: true,
            // The transaction learns how many invitations are acceptable only
            // under its own lock, so the ceiling is preassigned and a prefix is
            // consumed. Unused ids are never written anywhere.
            preassigned_membership_ids: (0..MAX_ACCEPTABLE_INVITATIONS)
                .map(|_| self.ids.next())
                .collect(),
            now: self.now(),
        };
        let memberships = AcceptInvitations::run(self.store.as_ref(), &command)
            .await
            .map_err(|error| control_error(cx, error))?;
        Ok(InvitationAcceptResult {
            memberships: memberships
                .iter()
                .map(|membership| {
                    membership_wire(&aex_control_app::ports::MembershipView {
                        membership: membership.clone(),
                        // Every membership this call produced belongs to the
                        // caller, so the address is the one already read above
                        // rather than a second read per row.
                        email: identity.email.clone(),
                    })
                })
                .collect::<WireResult<_>>()?,
        })
    }
}

impl WorkspacesApi for ControlService {
    async fn workspaces_list(
        &self,
        cx: &RequestContext,
        query: WorkspacesListQuery,
    ) -> WireResult<WorkspacePage> {
        let user_id = Self::user(cx)?;
        let organization_id = query.organization_id.map(raw);
        if let Some(organization_id) = organization_id {
            self.organization_access(cx, organization_id).await?;
        }
        let scope_id = organization_id.unwrap_or(user_id);
        let organization_filter = query
            .organization_id
            .map(|id| id.encode().as_str().to_owned())
            .unwrap_or_default();
        let binding = self.page_binding(
            cx,
            user_id,
            scope_id,
            &[("organizationId", &organization_filter)],
        );
        let page = self.page(&binding, query.cursor.as_ref(), query.limit)?;
        let result = self
            .store
            .list_workspace_views(&ListWorkspaces {
                user_id,
                organization_id,
                page,
            })
            .await
            .map_err(store_error)?;
        Ok(WorkspacePage {
            items: result
                .items
                .iter()
                .map(|view| self.workspace(view))
                .collect::<WireResult<_>>()?,
            next_cursor: self.cursor(&binding, result.next)?,
        })
    }

    async fn workspace_create(
        &self,
        cx: &RequestContext,
        body: WorkspaceCreateRequest,
    ) -> WireResult<Created<Workspace>> {
        let organization_id = raw(body.organization_id);
        self.require_admin(cx, organization_id).await?;
        if body.name.is_empty() || body.name.chars().count() > 128 {
            return Err(WireError::new(ErrorCode::InvalidRequest));
        }
        // The deployment may intentionally compose fewer than all launch
        // regions while the remaining regional stacks are unpublished. Check
        // the configured placement set before the central transaction so an
        // unsupported region cannot leave a hidden provisioning row behind.
        if !self.api_urls.contains_key(&body.region) {
            return Err(WireError::new(ErrorCode::InvalidRequest));
        }
        let workspace_id = self.ids.next();
        let operation_id = self.ids.next();
        let slug = slug(&body.name)?;
        let idempotency = self.idempotency(
            cx,
            &body,
            ScopeKind::Organization,
            organization_id,
            "/api/workspaces",
        )?;
        let command = BeginWorkspaceProvisionTx {
            preassigned_workspace_id: workspace_id,
            preassigned_operation_id: operation_id,
            organization_id,
            name: body.name,
            slug: slug.clone(),
            region: body.region,
            created_by_user_id: Self::user(cx)?,
            idempotency: idempotency.clone(),
            outbox: self.outbox(
                Topic::WorkspaceProvisionRequested,
                workspace_id.to_string(),
                organization_id,
                serde_json::json!({
                    "workspaceId": workspace_id,
                    "organizationId": organization_id,
                    "region": body.region.as_str(),
                    "operationId": operation_id,
                    "idempotencyId": idempotency.id,
                    "intentHash": hex(idempotency.intent_hash.as_bytes()),
                }),
            ),
            audit: self.audit(
                cx,
                AuditSpec {
                    organization_id: Some(organization_id),
                    workspace_id: Some(workspace_id),
                    action: "workspace.create",
                    resource_kind: ResourceKind::Workspace,
                    resource_id: Some(workspace_id),
                    operation_id: Some(operation_id),
                    detail: serde_json::json!({
                        "region": body.region.as_str(),
                        "slug": slug.as_str()
                    }),
                },
            )?,
            now: self.now(),
        };
        let created = CreateWorkspace::run(
            self.store.as_ref(),
            self.regional.as_ref(),
            &command,
            serde_json::json!({ "resourceId": workspace_id }),
            self.now(),
        )
        .await
        .map_err(|error| control_error(cx, error))?;
        let account = self
            .store
            .account_profile(organization_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::AccountStateUnavailable))?;
        Ok(Created(self.workspace(
            &aex_control_app::ports::WorkspaceView {
                workspace: created.workspace,
                account,
                workspace_epoch: 0,
            },
        )?))
    }

    async fn workspace_get(
        &self,
        _cx: &RequestContext,
        workspace_id: WorkspaceId,
    ) -> WireResult<Workspace> {
        let view = self.workspace_view(raw(workspace_id)).await?;
        self.workspace(&view)
    }

    async fn workspace_delete(
        &self,
        cx: &RequestContext,
        workspace_id: WorkspaceId,
        body: WorkspaceDeleteRequest,
    ) -> WireResult<Accepted> {
        if body.confirmation != workspace_id {
            return Err(WireError::new(ErrorCode::InvalidRequest));
        }
        let workspace_id = raw(workspace_id);
        let current = self.workspace_view(workspace_id).await?;
        let operation_id = raw(cx
            .operation_id
            .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest))?);
        let path = format!(
            "/api/workspaces/{}/deletions",
            wire::<WorkspaceId>(workspace_id)?
        );
        let command = BeginWorkspaceDeletionTx {
            workspace_id,
            organization_id: current.workspace.organization_id,
            operation_id,
            idempotency: self.idempotency(cx, &body, ScopeKind::Workspace, workspace_id, &path)?,
            outbox: self.outbox(
                Topic::WorkspaceDeleteRequested,
                operation_id.to_string(),
                current.workspace.organization_id,
                serde_json::json!({
                    "workspaceId": workspace_id,
                    "organizationId": current.workspace.organization_id,
                    "region": current.workspace.region.as_str(),
                    "operationId": operation_id,
                }),
            ),
            audit: self.audit(
                cx,
                AuditSpec {
                    organization_id: Some(current.workspace.organization_id),
                    workspace_id: Some(workspace_id),
                    action: "workspace.delete",
                    resource_kind: ResourceKind::Workspace,
                    resource_id: Some(workspace_id),
                    operation_id: Some(operation_id),
                    detail: serde_json::json!({ "to_status": "deleting" }),
                },
            )?,
            now: self.now(),
        };
        let (_, operation) = DeleteWorkspace::run(self.store.as_ref(), &command)
            .await
            .map_err(|error| control_error(cx, error))?;
        Ok(Accepted(operation_wire(
            &aex_control_app::ports::OperationView {
                operation,
                workspace_deleted_at: None,
            },
        )?))
    }
}

impl ApiKeysApi for ControlService {
    async fn api_keys_list(
        &self,
        cx: &RequestContext,
        query: ApiKeysListQuery,
    ) -> WireResult<ApiKeyPage> {
        let workspace_id = raw(query.workspace_id);
        let workspace = self.workspace_view(workspace_id).await?;
        self.require_admin(cx, workspace.workspace.organization_id)
            .await?;
        let principal_id = Self::user(cx)?;
        let binding = self.page_binding(cx, principal_id, workspace_id, &[]);
        let page = self.page(&binding, query.cursor.as_ref(), query.limit)?;
        let result = self
            .store
            .list_api_keys(&ListApiKeys { workspace_id, page })
            .await
            .map_err(store_error)?;
        Ok(ApiKeyPage {
            items: result
                .items
                .iter()
                .map(api_key_wire)
                .collect::<WireResult<_>>()?,
            next_cursor: self.cursor(&binding, result.next)?,
        })
    }

    async fn api_key_create(
        &self,
        cx: &RequestContext,
        body: ApiKeyCreateRequest,
    ) -> WireResult<Created<NewApiKey>> {
        if body.name.is_empty() || body.name.chars().count() > DomainApiKey::MAX_NAME_LEN {
            return Err(WireError::new(ErrorCode::InvalidRequest));
        }
        let workspace_id = raw(body.workspace_id);
        let workspace = self.workspace_view(workspace_id).await?;
        self.require_admin(cx, workspace.workspace.organization_id)
            .await?;
        if workspace.workspace.status != DomainWorkspaceStatus::Active {
            return Err(WireError::new(ErrorCode::ResourceConflict));
        }
        let requested = DomainScopeSet::from_strings(
            &body
                .scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>(),
        )
        .map_err(|_| WireError::new(ErrorCode::InvalidScope))?;
        let scopes = DomainApiKey::admissible_scopes(requested)
            .map_err(|_| WireError::new(ErrorCode::InvalidScope))?;
        let key_id = self.ids.next();
        let (secret, digest) = mint(
            CredentialKind::WorkspaceKey,
            Some(WorkspacePin {
                region: RegionCode::new(workspace.workspace.region),
                workspace: workspace_id,
            }),
            key_id,
            self.rng.as_ref(),
        );
        let (pepper_version, pepper) = self
            .peppers
            .active(PepperPurpose::ApiKey)
            .await
            .map_err(store_error)?;
        let keyed = verifier(&pepper, &digest);
        let command = CreateApiKeyTx {
            preassigned_id: key_id,
            workspace_id,
            organization_id: workspace.workspace.organization_id,
            name: body.name.clone(),
            scopes,
            region: workspace.workspace.region,
            verifier: *keyed.as_bytes(),
            pepper_version: pepper_version.get(),
            created_by_user_id: Self::user(cx)?,
            // Committed with the key itself. A region that never learned the key
            // exists cannot admit a request against it, so the announcement is
            // not allowed to be a second, separately-failing write.
            outbox: self.outbox(
                Topic::ApiKeyCreated,
                key_id.to_string(),
                workspace.workspace.organization_id,
                serde_json::json!({
                    "apiKeyId": key_id,
                    "workspaceId": workspace_id,
                    "organizationId": workspace.workspace.organization_id,
                    "region": workspace.workspace.region.as_str(),
                    "changedAt": timestamp(self.now())?,
                    "epoch": 0,
                }),
            ),
            idempotency: self.idempotency(
                cx,
                &body,
                ScopeKind::Workspace,
                workspace_id,
                "/api/api-keys",
            )?,
            audit: self.audit(
                cx,
                AuditSpec {
                    organization_id: Some(workspace.workspace.organization_id),
                    workspace_id: Some(workspace_id),
                    action: "api_key.create",
                    resource_kind: ResourceKind::ApiKey,
                    resource_id: Some(key_id),
                    operation_id: None,
                    detail: serde_json::json!({
                        "name": body.name,
                        "scopes": scopes.to_strings()
                    }),
                },
            )?,
            now: self.now(),
        };
        let created = CreateApiKey::run(self.store.as_ref(), &command)
            .await
            .map_err(|error| control_error(cx, error))?;
        if !created.first {
            return Err(WireError::new(ErrorCode::ApiKeySecretUnavailable));
        }
        let key = created.key;
        Ok(Created(NewApiKey {
            created_at: timestamp(key.created_at)?,
            id: wire(key.id)?,
            name: key.name,
            scopes: key.scopes.to_wire().as_slice().to_vec(),
            value: secret.expose().to_owned(),
            workspace_id: wire(key.workspace_id)?,
        }))
    }

    async fn api_key_revoke(
        &self,
        cx: &RequestContext,
        api_key_id: ApiKeyId,
    ) -> WireResult<NoContent> {
        let key_id = raw(api_key_id);
        let key = self
            .store
            .get_api_key(key_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        let expected_revision = cx.if_match.as_ref().map(parse_revision).transpose()?;
        let command = RevokeApiKeyTx {
            key_id,
            workspace_id: key.workspace_id,
            expected_revision,
            outbox: self.outbox(
                Topic::AuthorizationEpochChanged,
                key_id.to_string(),
                key.organization_id,
                serde_json::json!({
                    "apiKeyId": key_id,
                    "workspaceId": key.workspace_id,
                    "organizationId": key.organization_id,
                    "region": key.region.as_str(),
                    "changedAt": timestamp(self.now())?,
                }),
            ),
            audit: self.audit(
                cx,
                AuditSpec {
                    organization_id: Some(key.organization_id),
                    workspace_id: Some(key.workspace_id),
                    action: "api_key.revoke",
                    resource_kind: ResourceKind::ApiKey,
                    resource_id: Some(key_id),
                    operation_id: None,
                    detail: serde_json::json!({ "to_status": "revoked" }),
                },
            )?,
            now: self.now(),
        };
        RevokeApiKey::run(self.store.as_ref(), &command)
            .await
            .map_err(|error| control_error(cx, error))?;
        Ok(NoContent)
    }
}

impl CentralOperationsApi for ControlService {
    async fn central_operations_list(
        &self,
        cx: &RequestContext,
        query: CentralOperationsListQuery,
    ) -> WireResult<OperationPage> {
        let organization_id = raw(query.organization_id);
        self.organization_access(cx, organization_id).await?;
        let user_id = Self::user(cx)?;
        let kind = query.kind.map(|value| value.as_str().to_owned());
        let status = query.status.map(|value| value.as_str().to_owned());
        let binding = self.page_binding(
            cx,
            user_id,
            organization_id,
            &[
                ("kind", kind.as_deref().unwrap_or("")),
                ("status", status.as_deref().unwrap_or("")),
            ],
        );
        let page = self.page(&binding, query.cursor.as_ref(), query.limit)?;
        let domain_status = status.as_deref().and_then(DomainOperationStatus::parse);
        let result = self
            .store
            .list_operation_views(&ListOperations {
                organization_id,
                kind,
                status: domain_status,
                page,
            })
            .await
            .map_err(store_error)?;
        Ok(OperationPage {
            items: result
                .items
                .iter()
                .map(operation_wire)
                .collect::<WireResult<_>>()?,
            next_cursor: self.cursor(&binding, result.next)?,
        })
    }

    async fn central_operation_get(
        &self,
        _cx: &RequestContext,
        operation_id: OperationId,
    ) -> WireResult<Operation> {
        self.store
            .get_operation_view(raw(operation_id))
            .await
            .map_err(store_error)?
            .map(|view| operation_wire(&view))
            .transpose()?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))
    }

    async fn central_operation_cancel(
        &self,
        cx: &RequestContext,
        operation_id: OperationId,
        _body: EmptyRequest,
    ) -> WireResult<Operation> {
        let operation = CancelOperation::run(self.store.as_ref(), raw(operation_id), self.now())
            .await
            .map_err(|error| control_error(cx, error))?;
        operation_wire(&aex_control_app::ports::OperationView {
            operation,
            workspace_deleted_at: None,
        })
    }
}

impl BootstrapApi for ControlService {
    async fn dashboard_bootstrap_get(&self, cx: &RequestContext) -> WireResult<DashboardBootstrap> {
        let user_id = Self::user(cx)?;
        let organizations = self
            .store
            .list_organization_views(&ListOrganizations {
                user_id,
                page: PageRequest {
                    after: None,
                    limit: 100,
                },
            })
            .await
            .map_err(store_error)?;
        let workspaces = self
            .store
            .list_workspace_views(&ListWorkspaces {
                user_id,
                organization_id: None,
                page: PageRequest {
                    after: None,
                    limit: 500,
                },
            })
            .await
            .map_err(store_error)?;
        let email = self
            .store
            .user_identity(user_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::Forbidden))?
            .email;
        let mut accounts = Vec::with_capacity(organizations.items.len());
        for organization in &organizations.items {
            let profile = self
                .store
                .account_profile(organization.organization.id)
                .await
                .map_err(store_error)?
                .ok_or_else(|| WireError::new(ErrorCode::AccountStateUnavailable))?;
            accounts.push(OrganizationAccount {
                organization_id: wire(organization.organization.id)?,
                state: account_wire(&profile)?,
            });
        }
        Ok(DashboardBootstrap {
            accounts,
            email,
            generated_at: timestamp(self.now())?,
            organizations: organizations
                .items
                .iter()
                .map(organization_wire)
                .collect::<WireResult<_>>()?,
            user_id: wire(user_id)?,
            workspaces: workspaces
                .items
                .iter()
                .map(|view| self.workspace(view))
                .collect::<WireResult<_>>()?,
        })
    }
}

fn organization_wire(view: &aex_control_app::ports::OrganizationView) -> WireResult<Organization> {
    Ok(Organization {
        caller_role: role_wire(view.caller_role),
        created_at: timestamp(view.organization.created_at)?,
        id: wire(view.organization.id)?,
        name: view.organization.name.clone(),
        slug: view.organization.slug.as_str().to_owned(),
    })
}

fn membership_wire(view: &aex_control_app::ports::MembershipView) -> WireResult<Membership> {
    Ok(Membership {
        created_at: timestamp(view.membership.created_at)?,
        email: view.email.clone(),
        id: wire::<MembershipId>(view.membership.id)?,
        organization_id: wire(view.membership.organization_id)?,
        role: role_wire(view.membership.role),
        status: match view.membership.status {
            DomainMembershipStatus::Active => MembershipStatus::Active,
            DomainMembershipStatus::Removed => {
                return Err(WireError::new(ErrorCode::InternalError));
            }
        },
        user_id: wire::<UserId>(view.membership.user_id)?,
    })
}

fn invitation_wire(invitation: &DomainInvitation) -> WireResult<Invitation> {
    Ok(Invitation {
        created_at: timestamp(invitation.created_at)?,
        email: invitation.email.clone(),
        expires_at: timestamp(invitation.expires_at)?,
        id: wire::<InvitationId>(invitation.id)?,
        organization_id: wire(invitation.organization_id)?,
        resolved_at: invitation.resolved_at.map(timestamp).transpose()?,
        role: match invitation.role {
            OrgRole::Admin => InvitationRole::Admin,
            OrgRole::Member => InvitationRole::Member,
            OrgRole::Owner => return Err(WireError::new(ErrorCode::InternalError)),
        },
        status: match invitation.status {
            DomainInvitationStatus::Pending => InvitationStatus::Pending,
            DomainInvitationStatus::Accepted => InvitationStatus::Accepted,
            DomainInvitationStatus::Revoked => InvitationStatus::Revoked,
            DomainInvitationStatus::Expired => InvitationStatus::Expired,
        },
    })
}

fn api_key_wire(key: &DomainApiKey) -> WireResult<ApiKey> {
    Ok(ApiKey {
        created_at: timestamp(key.created_at)?,
        id: wire(key.id)?,
        name: key.name.clone(),
        revoked_at: key.revoked_at.map(timestamp).transpose()?,
        scopes: key.scopes.to_wire().as_slice().to_vec(),
        workspace_id: wire(key.workspace_id)?,
    })
}

fn workspace_wire(
    view: &aex_control_app::ports::WorkspaceView,
    api_urls: &BTreeMap<Region, HttpsUrl>,
) -> WireResult<Workspace> {
    let workspace = &view.workspace;
    let status = match workspace.status {
        DomainWorkspaceStatus::Active => WorkspaceStatus::Active,
        DomainWorkspaceStatus::Deleting => WorkspaceStatus::Deleting,
        DomainWorkspaceStatus::Provisioning => return Err(WireError::new(ErrorCode::NotFound)),
        DomainWorkspaceStatus::Deleted => return Err(WireError::new(ErrorCode::Gone)),
    };
    Ok(Workspace {
        api_url: api_urls
            .get(&workspace.region)
            .cloned()
            .ok_or_else(|| WireError::new(ErrorCode::InternalError))?,
        created_at: timestamp(workspace.created_at)?,
        deletion_operation_id: workspace.deletion_operation_id.map(wire).transpose()?,
        id: wire(workspace.id)?,
        name: workspace.name.clone(),
        operational_state: WorkspaceOperationalState {
            inherited_from: OperationalStateSource::Account,
            organization_id: wire(workspace.organization_id)?,
            state: account_wire(&view.account)?,
        },
        organization_id: wire(workspace.organization_id)?,
        region: workspace.region,
        slug: workspace.slug.as_str().to_owned(),
        status,
    })
}

/// Projects an account through the one shared mapping and maps its refusals.
///
/// There is no second derivation here and there must never be one: the mapping
/// lives in `aex-control-domain` precisely so this plane and the region cannot
/// answer the same question two ways.
fn account_wire(projection: &AccountProjection) -> WireResult<AccountOperationalState> {
    account_operational_state(&projection.profile).map_err(|error| match error {
        AccountProjectionError::Unavailable => WireError::new(ErrorCode::AccountStateUnavailable),
        AccountProjectionError::MissingPauseCause
        | AccountProjectionError::UnknownPauseCause(_)
        | AccountProjectionError::UnrepresentableInstant => {
            WireError::new(ErrorCode::InternalError)
        }
    })
}

fn operation_wire(view: &aex_control_app::ports::OperationView) -> WireResult<Operation> {
    let operation = &view.operation;
    if operation.kind != aex_control_domain::OperationKind::WorkspaceDelete {
        return Err(WireError::new(ErrorCode::InternalError));
    }
    let workspace_id = operation
        .workspace_id
        .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
    let status = match operation.status {
        DomainOperationStatus::Queued => OperationStatus::Queued,
        DomainOperationStatus::Running => OperationStatus::Running,
        DomainOperationStatus::Succeeded => OperationStatus::Succeeded,
        DomainOperationStatus::Failed => OperationStatus::Failed,
        DomainOperationStatus::Cancelled => OperationStatus::Cancelled,
    };
    let result = if operation.status == DomainOperationStatus::Succeeded {
        Some(OperationResult::WorkspaceDelete(WorkspaceTombstone {
            deleted_at: timestamp(
                view.workspace_deleted_at
                    .ok_or_else(|| WireError::new(ErrorCode::InternalError))?,
            )?,
            operation_id: wire(operation.id)?,
            organization_id: wire(operation.organization_id)?,
            workspace_id: wire(workspace_id)?,
        }))
    } else {
        None
    };
    Ok(Operation {
        cancelable: false,
        committed_at: if operation.status == DomainOperationStatus::Succeeded {
            operation.terminal_at.map(timestamp).transpose()?
        } else {
            None
        },
        created_at: timestamp(operation.created_at)?,
        error: None,
        id: wire(operation.id)?,
        kind: OperationKind::WorkspaceDelete,
        progress: None,
        result,
        session_id: None,
        started_at: operation.started_at.map(timestamp).transpose()?,
        status,
        terminal_at: operation.terminal_at.map(timestamp).transpose()?,
        updated_at: timestamp(operation.updated_at)?,
        workspace_id: wire(workspace_id)?,
    })
}

const fn role_wire(role: OrgRole) -> OrganizationRole {
    match role {
        OrgRole::Owner => OrganizationRole::Owner,
        OrgRole::Admin => OrganizationRole::Admin,
        OrgRole::Member => OrganizationRole::Member,
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Result::map_err supplies an owned store error"
)]
fn store_error(error: StoreError) -> WireError {
    match error {
        StoreError::NotFound => WireError::new(ErrorCode::NotFound),
        StoreError::Unavailable => WireError::new(ErrorCode::AccountStateUnavailable),
        StoreError::Conflict { .. } => WireError::new(ErrorCode::ResourceConflict),
        StoreError::Unknown => WireError::new(ErrorCode::CommitOutcomeUnknown),
        StoreError::Decode(_) | StoreError::Fatal(_) | StoreError::PermissionDenied => {
            WireError::new(ErrorCode::InternalError)
        }
    }
}

/// A failed read of `identity.user`, on a route that reads no account state.
///
/// [`store_error`] maps an unreachable store to `account_state_unavailable`,
/// which is true for every route that resolves an organization and false for
/// `invitation_accept`, which resolves none and never touches `finance`.
/// Publishing that code here would name a stage the request never reached, so
/// an unreachable identity authority is `upstream_error` — retryable, and
/// accurate about what failed.
#[allow(
    clippy::needless_pass_by_value,
    reason = "Result::map_err supplies an owned store error"
)]
fn identity_read_error(error: StoreError) -> WireError {
    match error {
        StoreError::Unavailable | StoreError::Unknown => {
            WireError::new(ErrorCode::UpstreamError)
        }
        StoreError::Conflict { .. } => WireError::new(ErrorCode::ResourceConflict),
        StoreError::NotFound
        | StoreError::Decode(_)
        | StoreError::Fatal(_)
        | StoreError::PermissionDenied => WireError::new(ErrorCode::InternalError),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the route-aware map_err closures supply an owned application error"
)]
fn control_error(cx: &RequestContext, error: ControlError) -> WireError {
    match error {
        ControlError::Conflict { .. } => WireError::new(ErrorCode::ResourceConflict),
        ControlError::NotFound => WireError::new(ErrorCode::NotFound),
        ControlError::IntentConflict => match route(cx.route).idempotency {
            aex_wire::idempotency::IdempotencyKind::OperationId => {
                WireError::new(ErrorCode::OperationIdempotencyConflict)
            }
            _ => WireError::new(ErrorCode::IdempotencyConflict),
        },
        ControlError::InFlight => WireError::new(ErrorCode::IdempotencyInFlight),
        ControlError::LastOwnerRequired => WireError::new(ErrorCode::LastOwnerRequired),
        ControlError::NotCancelable => WireError::new(ErrorCode::OperationNotCancelable),
        ControlError::DeletionInProgress => WireError::new(ErrorCode::DeletionInProgress),
        ControlError::WorkspaceProvisionPending => {
            WireError::new(ErrorCode::WorkspaceProvisionPending)
        }
        ControlError::Unavailable => WireError::new(ErrorCode::UpstreamError),
        ControlError::CommitOutcomeUnknown { .. } => {
            WireError::new(ErrorCode::CommitOutcomeUnknown)
        }
        ControlError::Fatal(_) => WireError::new(ErrorCode::InternalError),
    }
}

fn slug(name: &str) -> WireResult<Slug> {
    let mut value = String::with_capacity(name.len().min(64));
    let mut separator = false;
    for byte in name.bytes() {
        let byte = byte.to_ascii_lowercase();
        if byte.is_ascii_alphanumeric() {
            if separator && !value.is_empty() && value.len() < 64 {
                value.push('-');
            }
            separator = false;
            if value.len() < 64 {
                value.push(char::from(byte));
            }
        } else {
            separator = true;
        }
    }
    while value.ends_with('-') {
        value.pop();
    }
    if value.len() < 3 {
        value.push_str("-aex");
    }
    value.truncate(64);
    while value.ends_with('-') {
        value.pop();
    }
    Slug::parse(&value).map_err(|_| WireError::new(ErrorCode::InvalidRequest))
}

fn raw<I: PrefixedId>(id: I) -> Uuid {
    Uuid::from_bytes(*id.uuid7().as_bytes())
}

fn wire<I: PrefixedId>(id: Uuid) -> WireResult<I> {
    Uuid7::from_bytes(*id.as_bytes())
        .map(I::from_uuid7)
        .map_err(|_| WireError::new(ErrorCode::InternalError))
}

fn timestamp(value: OffsetDateTime) -> WireResult<Timestamp> {
    Timestamp::from_datetime_trunc_ms(value).map_err(|_| WireError::new(ErrorCode::InternalError))
}

fn millis(value: OffsetDateTime) -> i64 {
    i64::try_from(value.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX)
}

fn parse_revision(etag: &ETag) -> WireResult<u64> {
    etag.as_str()
        .strip_prefix("\"key-")
        .and_then(|value| value.strip_suffix('"'))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| WireError::new(ErrorCode::PreconditionFailed))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::ControlService;
    use aex_control_domain::OrgRole;
    use aex_wire::error::ErrorCode;

    #[test]
    fn api_key_handler_role_gate_rejects_a_member_but_accepts_admins_and_owners() {
        let denied = ControlService::admit_admin_role(OrgRole::Member)
            .expect_err("a member cannot perform an admin operation");
        assert_eq!(denied.code, ErrorCode::Forbidden);
        assert!(ControlService::admit_admin_role(OrgRole::Admin).is_ok());
        assert!(ControlService::admit_admin_role(OrgRole::Owner).is_ok());
    }

    #[test]
    fn a_handler_gates_the_account_state_only_where_the_edge_could_not() {
        use aex_wire::routes::{RouteId, route};

        // The edge resolves these routes' organization from the path and gates
        // the account state there. `memberships_list` re-read it anyway, and
        // since the route declares neither of the gate's two codes, the refusal
        // the edge had already made came back as `500 internal_error`.
        for id in [RouteId::MembershipsList, RouteId::InvitationCreate] {
            assert!(
                !ControlService::handler_owns_account_gate(id).expect("a central route"),
                "`{}` is gated at the edge",
                route(id).operation_id
            );
            assert!(
                !route(id).pause_exempt,
                "`{}` would not be gated at all otherwise",
                route(id).operation_id
            );
            for code in [ErrorCode::AccountPaused, ErrorCode::AccountStateUnavailable] {
                assert!(
                    !route(id).declares(code),
                    "`{}` declares `{}`, so the handler could gate after all",
                    route(id).operation_id,
                    code.as_str()
                );
            }
        }

        // These take their organization from a query parameter or a body, so
        // the edge cannot resolve it and defers the gate here. Each publishes
        // both outcomes, which is what makes gating in the handler legal.
        for id in [
            RouteId::CentralOperationsList,
            RouteId::ApiKeysList,
            RouteId::ApiKeyCreate,
        ] {
            assert!(
                ControlService::handler_owns_account_gate(id).expect("a central route"),
                "`{}` has no edge gate to inherit",
                route(id).operation_id
            );
            for code in [ErrorCode::AccountPaused, ErrorCode::AccountStateUnavailable] {
                assert!(
                    route(id).declares(code),
                    "`{}` gates in the handler without declaring `{}`",
                    route(id).operation_id,
                    code.as_str()
                );
            }
        }
    }
}
