//! Queue/schedule worker behavior and real external adapters.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_control_app::ports::{
    ClaimDueOperations, ClaimOutbox, ControlStore, ControlViewStore, FinishWorkspaceProvisionTx,
    GcExpired, ProvisionWorkspaceRequest, RegionalControlPort, RequestId, StoreError, TxOutcome,
};
use aex_control_domain::{
    ActorKind, AuditEvent, AuditOutcome, Operation, OperationKind, OperationStatus, OutboxMessage,
    ResourceKind, Topic, WorkspaceStatus,
};
use aex_identity_app::ports::Clock;
use aex_session_dynamodb::projection_write::{
    PlacementWrite, ProfileWrite, ProjectionWriter, RevocationWrite,
};
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

pub trait Store: ControlStore + ControlViewStore {}
impl<T: ControlStore + ControlViewStore> Store for T {}

#[async_trait]
pub trait Mail: Send + Sync {
    async fn send(&self, to: &str, subject: &str, text: &str) -> Result<(), String>;
}

#[async_trait]
pub trait SigningAdmin: Send + Sync {
    async fn confirm_published(&self, secret_ref: &str) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct SesMail {
    client: aws_sdk_sesv2::Client,
    from: String,
}

impl SesMail {
    #[must_use]
    pub fn new(client: aws_sdk_sesv2::Client, from: String) -> Self {
        Self { client, from }
    }
}

#[async_trait]
impl Mail for SesMail {
    async fn send(&self, to: &str, subject: &str, text: &str) -> Result<(), String> {
        use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message};

        let subject = Content::builder()
            .data(subject)
            .charset("UTF-8")
            .build()
            .map_err(|error| error.to_string())?;
        let text = Content::builder()
            .data(text)
            .charset("UTF-8")
            .build()
            .map_err(|error| error.to_string())?;
        let message = Message::builder()
            .subject(subject)
            .body(Body::builder().text(text).build())
            .build();
        self.client
            .send_email()
            .from_email_address(&self.from)
            .destination(Destination::builder().to_addresses(to).build())
            .content(EmailContent::builder().simple(message).build())
            .send()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct SecretsSigningAdmin {
    client: aws_sdk_secretsmanager::Client,
    prefix: String,
}

impl SecretsSigningAdmin {
    #[must_use]
    pub fn new(client: aws_sdk_secretsmanager::Client, prefix: String) -> Self {
        Self { client, prefix }
    }

    pub async fn probe(&self) -> Result<(), String> {
        self.client
            .list_secrets()
            .max_results(1)
            .filters(
                aws_sdk_secretsmanager::types::Filter::builder()
                    .key(aws_sdk_secretsmanager::types::FilterNameStringType::Name)
                    .values(&self.prefix)
                    .build(),
            )
            .send()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl SigningAdmin for SecretsSigningAdmin {
    async fn confirm_published(&self, secret_ref: &str) -> Result<(), String> {
        if !secret_ref.starts_with(&self.prefix) {
            return Err("signing secret is outside the configured prefix".to_owned());
        }
        self.client
            .describe_secret()
            .secret_id(secret_ref)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub struct Worker {
    store: Arc<dyn Store>,
    regional: Arc<dyn RegionalControlPort>,
    projections: BTreeMap<Region, ProjectionWriter>,
    mail: Arc<dyn Mail>,
    signing: Arc<dyn SigningAdmin>,
    clock: Arc<dyn Clock>,
    owner: String,
    batch: u32,
    lease: Duration,
}

impl Worker {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        regional: Arc<dyn RegionalControlPort>,
        projections: BTreeMap<Region, ProjectionWriter>,
        mail: Arc<dyn Mail>,
        signing: Arc<dyn SigningAdmin>,
        clock: Arc<dyn Clock>,
        owner: String,
        batch: u32,
        lease: Duration,
    ) -> Self {
        Self {
            store,
            regional,
            projections,
            mail,
            signing,
            clock,
            owner,
            batch,
            lease,
        }
    }

    /// Drains a bounded unit of durable work. Queue messages are wakeups; the
    /// Aurora claims remain the authority across duplicate deliveries.
    pub async fn tick(&self) -> Result<(), String> {
        let now = self.clock.now();
        // Advancing operation fences before outbox dispatch makes every retry a
        // fresh fenced attempt while retaining the same workspace identity.
        let operations = self
            .store
            .claim_due_operations(&ClaimDueOperations {
                owner: self.owner.clone(),
                lease: self.lease,
                batch: self.batch,
                now,
            })
            .await
            .map_err(redacted_store)?;
        let mut operation_failed = false;
        for operation in operations {
            if self.recover_operation(&operation).await.is_err() {
                operation_failed = true;
            }
        }
        let messages = self
            .store
            .claim_outbox(&ClaimOutbox {
                owner: self.owner.clone(),
                lease: self.lease,
                batch: self.batch,
                now,
            })
            .await
            .map_err(redacted_store)?;
        for message in messages {
            match self.dispatch(&message).await {
                Ok(()) => self
                    .store
                    .mark_outbox_dispatched(message.id, self.clock.now())
                    .await
                    .map_err(redacted_store)?,
                Err(error) => {
                    let backoff = i64::from(2_u32.saturating_pow(message.attempts.min(8))).min(300);
                    self.store
                        .release_outbox(
                            message.id,
                            self.clock.now() + Duration::seconds(backoff),
                            &error,
                        )
                        .await
                        .map_err(redacted_store)?;
                }
            }
        }
        if operation_failed {
            Err("operation_recovery_failed".to_owned())
        } else {
            Ok(())
        }
    }

    pub async fn scheduled(&self) -> Result<(), String> {
        self.tick().await?;
        self.store
            .gc_expired(&GcExpired {
                now: self.clock.now(),
                batch: self.batch,
            })
            .await
            .map(|_| ())
            .map_err(redacted_store)
    }

    async fn dispatch(&self, message: &OutboxMessage) -> Result<(), String> {
        match message.topic {
            Topic::WorkspaceProvisionRequested => self.provision(message).await,
            Topic::WorkspaceDeleteRequested => self.delete(message).await,
            Topic::AccountStateChanged => self.account_changed(message).await,
            Topic::InvitationEmailRequested => self.invitation(message).await,
            Topic::AuthorizationEpochChanged => self.revocation(message).await,
            Topic::AuthorizationSigningKeyPublished => self.signing(message).await,
        }
    }

    async fn provision(&self, message: &OutboxMessage) -> Result<(), String> {
        let payload: ProvisionPayload = serde_json::from_value(message.payload.clone())
            .map_err(|_| "invalid_workspace_provision_payload".to_owned())?;
        let workspace = self
            .store
            .get_workspace(payload.workspace_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "workspace_not_found".to_owned())?;
        let operation = self
            .store
            .get_operation(payload.operation_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "operation_not_found".to_owned())?;
        if workspace.organization_id != payload.organization_id
            || workspace.region != payload.region
            || operation.workspace_id != Some(workspace.id)
            || hex(&operation.intent_hash) != payload.intent_hash
        {
            return Err("workspace_provision_payload_mismatch".to_owned());
        }
        if workspace.status == WorkspaceStatus::Provisioning {
            let answer = self
                .regional
                .provision_workspace(&ProvisionWorkspaceRequest {
                    workspace_id: workspace.id,
                    organization_id: workspace.organization_id,
                    region: workspace.region,
                    fence: operation.fence,
                    intent_hash: operation.intent_hash,
                })
                .await
                .map_err(|_| "regional_provision_unavailable".to_owned())?;
            if answer.workspace_id != workspace.id {
                return Err("regional_answered_another_workspace".to_owned());
            }
            let finish = FinishWorkspaceProvisionTx {
                workspace_id: workspace.id,
                operation_id: operation.id,
                fence: operation.fence,
                idempotency_id: payload.idempotency_id,
                response_body: serde_json::json!({ "resourceId": workspace.id }),
                audit: system_audit(
                    &workspace,
                    "workspace.provision.completed",
                    self.clock.now(),
                ),
                now: self.clock.now(),
            };
            known(
                self.store
                    .finish_workspace_provision(&finish)
                    .await
                    .map_err(redacted_store)?,
            )?;
        }
        self.project_workspace(payload.workspace_id, sequence(message.created_at))
            .await
    }

    async fn delete(&self, message: &OutboxMessage) -> Result<(), String> {
        let payload: DeletePayload = serde_json::from_value(message.payload.clone())
            .map_err(|_| "invalid_workspace_delete_payload".to_owned())?;
        let workspace = self
            .store
            .get_workspace(payload.workspace_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "workspace_not_found".to_owned())?;
        let operation = self
            .store
            .get_operation(payload.operation_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "operation_not_found".to_owned())?;
        if workspace.organization_id != payload.organization_id
            || workspace.region != payload.region
            || operation.workspace_id != Some(workspace.id)
        {
            return Err("workspace_delete_payload_mismatch".to_owned());
        }
        self.project_workspace(workspace.id, sequence(message.created_at))
            .await?;
        if workspace.status != WorkspaceStatus::Deleted
            && operation.status != OperationStatus::Succeeded
        {
            aex_control_app::use_cases::DeleteWorkspace::dispatch(
                self.store.as_ref(),
                self.regional.as_ref(),
                &workspace,
                &operation,
                self.clock.now(),
            )
            .await
            .map_err(|_| "regional_delete_unavailable".to_owned())?;
        }
        Ok(())
    }

    async fn account_changed(&self, message: &OutboxMessage) -> Result<(), String> {
        let payload: AccountStatePayload = serde_json::from_value(message.payload.clone())
            .map_err(|_| "invalid_account_state_payload".to_owned())?;
        let view = self
            .store
            .get_workspace_view(payload.workspace_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "workspace_projection_not_found".to_owned())?;
        if view.workspace.organization_id != payload.organization_id
            || view.workspace.region != payload.region
            || view.account.epoch < payload.account_epoch
            || view.account.revision < payload.account_revision
        {
            return Err("account_state_payload_mismatch".to_owned());
        }
        self.project_view(&view, payload.changed_at_ms).await
    }

    async fn invitation(&self, message: &OutboxMessage) -> Result<(), String> {
        let payload: InvitationPayload = serde_json::from_value(message.payload.clone())
            .map_err(|_| "invalid_invitation_payload".to_owned())?;
        self.mail
            .send(
                &payload.email,
                "You were invited to AEX",
                &format!(
                    "You were invited to organization {} as {}.",
                    payload.organization_id, payload.role
                ),
            )
            .await
    }

    async fn revocation(&self, message: &OutboxMessage) -> Result<(), String> {
        let payload: RevocationPayload = serde_json::from_value(message.payload.clone())
            .map_err(|_| "invalid_revocation_payload".to_owned())?;
        let writer = self
            .projections
            .get(&payload.region)
            .ok_or_else(|| "regional_projection_not_configured".to_owned())?;
        writer
            .put_revocation(&RevocationWrite {
                api_key: api_key(payload.api_key_id)?,
                revoked_at: timestamp(payload.revoked_at)?,
                epoch: payload.epoch,
            })
            .await
    }

    async fn signing(&self, message: &OutboxMessage) -> Result<(), String> {
        let payload: SigningPayload = serde_json::from_value(message.payload.clone())
            .map_err(|_| "invalid_signing_payload".to_owned())?;
        self.signing.confirm_published(&payload.secret_ref).await
    }

    async fn recover_operation(&self, operation: &Operation) -> Result<(), String> {
        match operation.kind {
            OperationKind::WorkspaceProvision => self.recover_provision(operation).await,
            OperationKind::WorkspaceDelete => self.recover_delete(operation).await,
        }
    }

    async fn recover_provision(&self, operation: &Operation) -> Result<(), String> {
        let workspace_id = operation
            .workspace_id
            .ok_or_else(|| "provision_operation_has_no_workspace".to_owned())?;
        let workspace = self
            .store
            .get_workspace(workspace_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "workspace_not_found".to_owned())?;
        if workspace.organization_id != operation.organization_id {
            return Err("provision_operation_tenant_mismatch".to_owned());
        }
        if workspace.status == WorkspaceStatus::Provisioning {
            let answer = self
                .regional
                .provision_workspace(&ProvisionWorkspaceRequest {
                    workspace_id,
                    organization_id: workspace.organization_id,
                    region: workspace.region,
                    fence: operation.fence,
                    intent_hash: operation.intent_hash,
                })
                .await
                .map_err(|_| "regional_provision_unavailable".to_owned())?;
            if answer.workspace_id != workspace_id {
                return Err("regional_answered_another_workspace".to_owned());
            }
            let idempotency_id = self
                .store
                .idempotency_id_for_operation(operation.id)
                .await
                .map_err(redacted_store)?
                .ok_or_else(|| "operation_idempotency_not_found".to_owned())?;
            known(
                self.store
                    .finish_workspace_provision(&FinishWorkspaceProvisionTx {
                        workspace_id,
                        operation_id: operation.id,
                        fence: operation.fence,
                        idempotency_id,
                        response_body: serde_json::json!({ "resourceId": workspace_id }),
                        audit: system_audit(
                            &workspace,
                            "workspace.provision.completed",
                            self.clock.now(),
                        ),
                        now: self.clock.now(),
                    })
                    .await
                    .map_err(redacted_store)?,
            )?;
        }
        self.project_workspace(workspace_id, sequence(operation.updated_at))
            .await
    }

    async fn recover_delete(&self, operation: &Operation) -> Result<(), String> {
        let workspace_id = operation
            .workspace_id
            .ok_or_else(|| "delete_operation_has_no_workspace".to_owned())?;
        let workspace = self
            .store
            .get_workspace(workspace_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "workspace_not_found".to_owned())?;
        if workspace.organization_id != operation.organization_id {
            return Err("delete_operation_tenant_mismatch".to_owned());
        }
        self.project_workspace(workspace_id, sequence(operation.updated_at))
            .await?;
        if workspace.status != WorkspaceStatus::Deleted {
            aex_control_app::use_cases::DeleteWorkspace::dispatch(
                self.store.as_ref(),
                self.regional.as_ref(),
                &workspace,
                operation,
                self.clock.now(),
            )
            .await
            .map_err(|_| "regional_delete_unavailable".to_owned())?;
        }
        Ok(())
    }

    async fn project_workspace(
        &self,
        workspace_id: Uuid,
        feed_sequence: u64,
    ) -> Result<(), String> {
        let view = self
            .store
            .get_workspace_view(workspace_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "workspace_projection_not_found".to_owned())?;
        self.project_view(&view, feed_sequence).await
    }

    async fn project_view(
        &self,
        view: &aex_control_app::ports::WorkspaceView,
        feed_sequence: u64,
    ) -> Result<(), String> {
        let status = match view.workspace.status {
            WorkspaceStatus::Provisioning => return Err("workspace_not_projectable_yet".to_owned()),
            WorkspaceStatus::Deleting | WorkspaceStatus::Deleted => "deleting",
            WorkspaceStatus::Active
                if view.account.state == aex_control_domain::AccountState::PausedTopUpRequired =>
            {
                "paused"
            }
            WorkspaceStatus::Active => "active",
        };
        let writer = self
            .projections
            .get(&view.workspace.region)
            .ok_or_else(|| "regional_projection_not_configured".to_owned())?;
        writer
            .put_profile(&ProfileWrite {
                workspace: workspace(view.workspace.id)?,
                name: view.workspace.name.clone(),
                slug: view.workspace.slug.as_str().to_owned(),
                created_at: timestamp(view.workspace.created_at)?,
            })
            .await?;
        writer
            .put_placement(&PlacementWrite {
                workspace: workspace(view.workspace.id)?,
                organization: organization(view.workspace.organization_id)?,
                region: view.workspace.region,
                status: status.to_owned(),
                key_epoch: 0,
                account_epoch: view.account.epoch,
                revocation_epoch: view.workspace_epoch,
                feed_sequence,
                updated_at: timestamp(self.clock.now())?,
            })
            .await
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProvisionPayload {
    workspace_id: Uuid,
    organization_id: Uuid,
    region: Region,
    operation_id: Uuid,
    idempotency_id: Uuid,
    intent_hash: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeletePayload {
    workspace_id: Uuid,
    organization_id: Uuid,
    region: Region,
    operation_id: Uuid,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountStatePayload {
    workspace_id: Uuid,
    organization_id: Uuid,
    region: Region,
    account_epoch: u64,
    account_revision: u64,
    changed_at_ms: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InvitationPayload {
    #[allow(dead_code)]
    invitation_id: Uuid,
    organization_id: Uuid,
    email: String,
    role: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevocationPayload {
    api_key_id: Uuid,
    #[allow(dead_code)]
    workspace_id: Uuid,
    #[allow(dead_code)]
    organization_id: Uuid,
    region: Region,
    revoked_at: OffsetDateTime,
    epoch: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SigningPayload {
    secret_ref: String,
}

fn known<T>(outcome: TxOutcome<T>) -> Result<T, String> {
    match outcome {
        TxOutcome::Committed(value) | TxOutcome::Replayed(value) => Ok(value),
        TxOutcome::IntentConflict => Err("intent_conflict".to_owned()),
        TxOutcome::Unknown(_) => Err("commit_outcome_unknown".to_owned()),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Result::map_err supplies an owned store error"
)]
fn redacted_store(error: StoreError) -> String {
    match error {
        StoreError::Conflict { .. } => "store_conflict",
        StoreError::NotFound => "store_not_found",
        StoreError::Unavailable => "store_unavailable",
        StoreError::Unknown => "store_outcome_unknown",
        StoreError::Decode(_) => "store_decode_failure",
        StoreError::PermissionDenied => "store_permission_denied",
        StoreError::Fatal(_) => "store_fatal",
    }
    .to_owned()
}

fn sequence(at: OffsetDateTime) -> u64 {
    u64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or_default()
}

fn timestamp(at: OffsetDateTime) -> Result<Timestamp, String> {
    let millis = i64::try_from(at.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| "timestamp_out_of_range".to_owned())?;
    Timestamp::from_unix_millis(millis).map_err(|_| "timestamp_out_of_range".to_owned())
}

fn workspace(id: Uuid) -> Result<WorkspaceId, String> {
    Uuid7::from_bytes(*id.as_bytes())
        .map(WorkspaceId::from_uuid7)
        .map_err(|_| "workspace_id_not_uuid7".to_owned())
}

fn organization(id: Uuid) -> Result<OrganizationId, String> {
    Uuid7::from_bytes(*id.as_bytes())
        .map(OrganizationId::from_uuid7)
        .map_err(|_| "organization_id_not_uuid7".to_owned())
}

fn api_key(id: Uuid) -> Result<ApiKeyId, String> {
    Uuid7::from_bytes(*id.as_bytes())
        .map(ApiKeyId::from_uuid7)
        .map_err(|_| "api_key_id_not_uuid7".to_owned())
}

fn hex(intent: &aex_control_domain::IntentHash) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut rendered = String::with_capacity(64);
    for byte in intent.as_bytes() {
        rendered.push(char::from(DIGITS[usize::from(byte >> 4)]));
        rendered.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    rendered
}

fn system_audit(
    workspace: &aex_control_domain::Workspace,
    action: &str,
    now: OffsetDateTime,
) -> AuditEvent {
    AuditEvent {
        id: Uuid::nil(),
        organization_id: Some(workspace.organization_id),
        workspace_id: Some(workspace.id),
        actor_kind: ActorKind::System,
        actor_id: None,
        action: action.to_owned(),
        resource_kind: ResourceKind::Workspace,
        resource_id: Some(workspace.id),
        outcome: AuditOutcome::Allowed,
        request_id: RequestId::new("central-control-worker").as_str().to_owned(),
        operation_id: Some(workspace.provision_operation_id),
        detail: serde_json::json!({ "to_status": "active" }),
        occurred_at: now,
    }
}
