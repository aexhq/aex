//! API keys and the one-account dashboard bootstrap.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_central_http::cursor::{PageBinding, next_cursor, page_request};
use aex_control_app::ports::{
    Clock, ControlStore, ControlViewStore, CreateApiKeyTx, IdempotencyRecordKey, ListApiKeys,
    ListOrganizations, ListWorkspaces, PageRequest, RevokeApiKeyTx, StoreError,
};
use aex_control_app::{ControlError, CreateApiKey, RevokeApiKey};
use aex_control_domain::{
    AccountProjectionError, ApiKey as DomainApiKey, AuditEvent, AuditOutcome, CursorSecret,
    IdempotencyKeyKind, IntentHash, OrgRole, OutboxMessage, PrincipalKindTag, ResourceKind,
    ScopeKind, ScopeSet as DomainScopeSet, Topic, WorkspaceStatus as DomainWorkspaceStatus,
    account_operational_state,
};
use aex_identity_app::ports::{IdFactory, PepperKeystore, PepperPurpose};
use aex_identity_domain::SecretRng;
use aex_identity_domain::credential::{CredentialKind, RegionCode, WorkspacePin, mint, verifier};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{AccountId, ApiKeyId, PrefixedId, UserId, Uuid7, WorkspaceId};
use aex_wire::models::{
    ApiKey, ApiKeyCreateRequest, ApiKeyPage, ApiKeysListQuery, DashboardBootstrap, NewApiKey,
    Workspace, WorkspaceOperationalState,
};
use aex_wire::routes::route;
use aex_wire::server::{ApiKeysApi, BootstrapApi, Created, NoContent, RequestContext};
use aex_wire::types::{ETag, HttpsUrl, Region, Timestamp};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Store surface used by the slim central public boundary.
pub trait Store: ControlStore + ControlViewStore {}
impl<T: ControlStore + ControlViewStore> Store for T {}

/// API-key and fixed-workspace service.
pub struct ControlService {
    store: Arc<dyn Store>,
    peppers: Arc<dyn PepperKeystore>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdFactory>,
    rng: Arc<dyn SecretRng>,
    cursor_secret: Arc<CursorSecret>,
    region: Region,
    api_urls: BTreeMap<Region, HttpsUrl>,
}

impl ControlService {
    /// Composes API-key and bootstrap handlers over the fixed personal workspace.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        peppers: Arc<dyn PepperKeystore>,
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
        i64::try_from(self.now().unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX)
    }
    fn user(cx: &RequestContext) -> WireResult<Uuid> {
        match cx.principal {
            PrincipalScope::Account { user, .. } => Ok(raw(user)),
            PrincipalScope::WorkspaceKey { .. } => Err(WireError::new(ErrorCode::Forbidden)),
        }
    }
    async fn fixed_workspace(
        &self,
        user: Uuid,
    ) -> WireResult<aex_control_app::ports::WorkspaceView> {
        let organizations = self
            .store
            .list_organization_views(&ListOrganizations {
                user_id: user,
                page: PageRequest {
                    after: None,
                    limit: 2,
                },
            })
            .await
            .map_err(store_error)?;
        let organization = organizations.items.first().ok_or_else(|| {
            WireError::new(ErrorCode::AccountStateUnavailable)
                .with_message("the personal account has not been provisioned")
        })?;
        if organizations.items.len() != 1 {
            return Err(WireError::new(ErrorCode::InternalError)
                .with_message("a launch user must resolve to exactly one personal account"));
        }
        let workspaces = self
            .store
            .list_workspace_views(&ListWorkspaces {
                user_id: user,
                organization_id: Some(organization.organization.id),
                page: PageRequest {
                    after: None,
                    limit: 2,
                },
            })
            .await
            .map_err(store_error)?;
        if workspaces.items.len() != 1 {
            return Err(WireError::new(ErrorCode::AccountStateUnavailable)
                .with_message("the fixed workspace has not been provisioned"));
        }
        Ok(workspaces.items.into_iter().next().expect("one workspace"))
    }
    async fn require_owner(
        &self,
        cx: &RequestContext,
        workspace_id: Uuid,
    ) -> WireResult<aex_control_app::ports::WorkspaceView> {
        let workspace = self
            .store
            .get_workspace_view(workspace_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        let organization = self
            .store
            .get_organization_view(workspace.workspace.organization_id, Self::user(cx)?)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::Forbidden))?;
        if !organization.caller_role.at_least(OrgRole::Admin) {
            return Err(WireError::new(ErrorCode::Forbidden));
        }
        Ok(workspace)
    }
    fn binding(&self, cx: &RequestContext, workspace: Uuid) -> WireResult<PageBinding> {
        Ok(PageBinding {
            endpoint: route(cx.route).template,
            principal_id: Self::user(cx)?,
            scope_id: workspace,
            region: self.region,
            filter_hash: PageBinding::filter_hash(&[]),
            snapshot_ms: self.now_ms(),
        })
    }
    fn outbox(
        &self,
        topic: Topic,
        key: String,
        organization: Uuid,
        payload: serde_json::Value,
    ) -> OutboxMessage {
        let now = self.now();
        OutboxMessage {
            id: self.ids.next(),
            topic,
            dedupe_key: key,
            group_key: organization.to_string(),
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
    fn audit(
        &self,
        cx: &RequestContext,
        action: &str,
        organization: Uuid,
        workspace: Uuid,
        resource: Uuid,
    ) -> WireResult<AuditEvent> {
        Ok(AuditEvent {
            id: self.ids.next(),
            organization_id: Some(organization),
            workspace_id: Some(workspace),
            actor_kind: aex_control_domain::ActorKind::User,
            actor_id: Some(Self::user(cx)?),
            action: action.to_owned(),
            resource_kind: ResourceKind::ApiKey,
            resource_id: Some(resource),
            outcome: AuditOutcome::Allowed,
            request_id: cx.request_id.as_str().to_owned(),
            operation_id: None,
            detail: serde_json::json!({}),
            occurred_at: self.now(),
        })
    }
    fn idempotency(
        &self,
        cx: &RequestContext,
        workspace: Uuid,
        body: &ApiKeyCreateRequest,
    ) -> WireResult<IdempotencyRecordKey> {
        let canonical =
            aex_wire::to_jcs_bytes(body).map_err(|_| WireError::new(ErrorCode::InvalidRequest))?;
        let intent = aex_wire::intent_digest(
            cx.route,
            &aex_wire::routes::PathBinding::default(),
            Some(&canonical),
        );
        Ok(IdempotencyRecordKey {
            id: self.ids.next(),
            key_kind: IdempotencyKeyKind::IdempotencyKey,
            key_value: cx
                .idempotency_key
                .as_ref()
                .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest))?
                .as_str()
                .to_owned(),
            principal_kind: PrincipalKindTag::AccountActor,
            principal_id: Self::user(cx)?,
            scope_kind: ScopeKind::Workspace,
            scope_id: workspace,
            method: route(cx.route).method,
            route: route(cx.route).template.to_owned(),
            intent_hash: IntentHash::from_bytes(*intent.as_bytes()),
            expires_at: self.now() + Duration::days(1),
        })
    }
}

impl ApiKeysApi for ControlService {
    async fn api_keys_list(
        &self,
        cx: &RequestContext,
        query: ApiKeysListQuery,
    ) -> WireResult<ApiKeyPage> {
        let workspace = raw(query.workspace_id);
        self.require_owner(cx, workspace).await?;
        let binding = self.binding(cx, workspace)?;
        let page = page_request(
            &self.cursor_secret,
            &binding,
            query.cursor.as_ref().map(aex_wire::Cursor::as_str),
            query.limit,
            self.now_ms(),
        )
        .map_err(aex_central_http::error::EdgeError::into_wire)?;
        let result = self
            .store
            .list_api_keys(&ListApiKeys {
                workspace_id: workspace,
                page,
            })
            .await
            .map_err(store_error)?;
        Ok(ApiKeyPage {
            items: result
                .items
                .iter()
                .map(api_key_wire)
                .collect::<WireResult<_>>()?,
            next_cursor: next_cursor(&self.cursor_secret, &binding, result.next, self.now_ms())
                .map(|raw| {
                    aex_wire::Cursor::parse(&raw)
                        .map_err(|_| WireError::new(ErrorCode::InternalError))
                })
                .transpose()?,
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
        let workspace = self.require_owner(cx, workspace_id).await?;
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
        let command = CreateApiKeyTx { preassigned_id: key_id, workspace_id, organization_id: workspace.workspace.organization_id,
            name: body.name.clone(), scopes, region: workspace.workspace.region, verifier: *keyed.as_bytes(), pepper_version: pepper_version.get(),
            created_by_user_id: Self::user(cx)?, outbox: self.outbox(Topic::ApiKeyCreated, key_id.to_string(), workspace.workspace.organization_id,
                serde_json::json!({"apiKeyId": key_id, "workspaceId": workspace_id, "organizationId": workspace.workspace.organization_id,
                    "region": workspace.workspace.region.as_str(), "changedAt": timestamp(self.now())?, "epoch": 0})),
            idempotency: self.idempotency(cx, workspace_id, &body)?, audit: self.audit(cx, "api_key.create", workspace.workspace.organization_id, workspace_id, key_id)?, now: self.now() };
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
        self.require_owner(cx, key.workspace_id).await?;
        let command = RevokeApiKeyTx { key_id, workspace_id: key.workspace_id, expected_revision: cx.if_match.as_ref().map(parse_revision).transpose()?,
            outbox: self.outbox(Topic::AuthorizationEpochChanged, key_id.to_string(), key.organization_id,
                serde_json::json!({"apiKeyId": key_id, "workspaceId": key.workspace_id, "organizationId": key.organization_id,
                    "region": key.region.as_str(), "changedAt": timestamp(self.now())?})),
            audit: self.audit(cx, "api_key.revoke", key.organization_id, key.workspace_id, key_id)?, now: self.now() };
        RevokeApiKey::run(self.store.as_ref(), &command)
            .await
            .map_err(|error| control_error(cx, error))?;
        Ok(NoContent)
    }
}

impl BootstrapApi for ControlService {
    async fn dashboard_bootstrap_get(&self, cx: &RequestContext) -> WireResult<DashboardBootstrap> {
        let user = Self::user(cx)?;
        let workspace = self.fixed_workspace(user).await?;
        let identity = self
            .store
            .user_identity(user)
            .await
            .map_err(store_error)?
            .ok_or_else(|| WireError::new(ErrorCode::Forbidden))?;
        let account = AccountId::from_uuid7(uuid7(workspace.workspace.organization_id)?);
        Ok(DashboardBootstrap {
            user_id: wire::<UserId>(user)?,
            email: identity.email,
            account_id: account,
            account_state: account_operational_state(&workspace.account.profile)
                .map_err(account_projection_error)?,
            workspace: Workspace {
                id: wire::<WorkspaceId>(workspace.workspace.id)?,
                name: workspace.workspace.name,
                region: workspace.workspace.region,
                api_url: self
                    .api_urls
                    .get(&workspace.workspace.region)
                    .cloned()
                    .ok_or_else(|| WireError::new(ErrorCode::InternalError))?,
                operational_state: WorkspaceOperationalState {
                    account_id: account,
                    state: account_operational_state(&workspace.account.profile)
                        .map_err(account_projection_error)?,
                },
                created_at: timestamp(workspace.workspace.created_at)?,
            },
            generated_at: timestamp(self.now())?,
        })
    }
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
fn account_projection_error(error: AccountProjectionError) -> WireError {
    match error {
        AccountProjectionError::Unavailable => WireError::new(ErrorCode::AccountStateUnavailable),
        _ => WireError::new(ErrorCode::InternalError),
    }
}
fn store_error(error: StoreError) -> WireError {
    match error {
        StoreError::NotFound => WireError::new(ErrorCode::NotFound),
        StoreError::Conflict { .. } => WireError::new(ErrorCode::ResourceConflict),
        StoreError::Unavailable => WireError::new(ErrorCode::AccountStateUnavailable),
        StoreError::Unknown => WireError::new(ErrorCode::CommitOutcomeUnknown),
        _ => WireError::new(ErrorCode::InternalError),
    }
}
fn control_error(cx: &RequestContext, error: ControlError) -> WireError {
    match error {
        ControlError::Conflict { .. } => WireError::new(ErrorCode::ResourceConflict),
        ControlError::NotFound => WireError::new(ErrorCode::NotFound),
        ControlError::IntentConflict => WireError::new(ErrorCode::IdempotencyConflict),
        ControlError::InFlight => WireError::new(ErrorCode::IdempotencyInFlight),
        ControlError::Unavailable => WireError::new(ErrorCode::UpstreamError),
        ControlError::CommitOutcomeUnknown { .. } => {
            WireError::new(ErrorCode::CommitOutcomeUnknown)
        }
        _ => {
            let _ = cx;
            WireError::new(ErrorCode::InternalError)
        }
    }
}
fn raw<I: PrefixedId>(id: I) -> Uuid {
    Uuid::from_bytes(*id.uuid7().as_bytes())
}
fn uuid7(id: Uuid) -> WireResult<Uuid7> {
    Uuid7::from_bytes(*id.as_bytes()).map_err(|_| WireError::new(ErrorCode::InternalError))
}
fn wire<I: PrefixedId>(id: Uuid) -> WireResult<I> {
    uuid7(id).map(I::from_uuid7)
}
fn timestamp(value: OffsetDateTime) -> WireResult<Timestamp> {
    Timestamp::from_datetime_trunc_ms(value).map_err(|_| WireError::new(ErrorCode::InternalError))
}
fn parse_revision(etag: &ETag) -> WireResult<u64> {
    etag.as_str()
        .strip_prefix("\"key-")
        .and_then(|v| v.strip_suffix('"'))
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| WireError::new(ErrorCode::PreconditionFailed))
}
