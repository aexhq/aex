//! Transaction-wake worker behavior and real external adapters.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use aex_control_app::ports::{
    ClaimOutbox, ControlStore, ControlViewStore, FinishWorkspaceProvisionTx, GcExpired,
    KeyMaterialReader, RequestId, StoreError, TxOutcome,
};
use aex_control_aurora::OutboxWakeTransactionStatus as Transaction;
use aex_control_domain::{
    ActorKind, AuditEvent, AuditOutcome, OutboxMessage, ResourceKind, ScopeSet, Topic,
    WorkspaceStatus,
};
use aex_identity_app::ports::Clock;
use aex_internal_contracts::assertion::AudienceSet;
use aex_session_dynamodb::projection_write::{
    KeyAuthorizationWrite, PlacementWrite, ProjectionWriter,
};
use aex_session_dynamodb::wire_pending::KeyAuthorizationState;
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Everything the drain reads and writes in the control authority.
#[async_trait]
pub trait Store: ControlStore + ControlViewStore + KeyMaterialReader {
    /// Resolves the pre-commit race and reads the exact anchor named by a wake.
    async fn outbox_wake_state(
        &self,
        anchor_id: Uuid,
        transaction_id: &str,
    ) -> Result<aex_control_aurora::OutboxWakeState, StoreError>;
}

#[async_trait]
impl Store for aex_control_aurora::AuroraControlStore {
    async fn outbox_wake_state(
        &self,
        anchor_id: Uuid,
        transaction_id: &str,
    ) -> Result<aex_control_aurora::OutboxWakeState, StoreError> {
        Self::outbox_wake_state(self, anchor_id, transaction_id).await
    }
}

/// The transaction-scoped wake accepted by Aurora before commit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutboxWake {
    /// Exact protocol marker.
    pub schema: String,
    /// One row inserted by the statement that emitted this wake.
    pub anchor_id: Uuid,
    /// Full transaction id returned by `pg_current_xact_id()`.
    pub transaction_id: String,
}

impl OutboxWake {
    /// The only accepted protocol marker.
    pub const SCHEMA: &'static str = "aex.control-outbox-wake.v1";

    fn validate(&self) -> Result<(), String> {
        if self.schema != Self::SCHEMA {
            return Err("unsupported_outbox_wake_schema".to_owned());
        }
        if self.transaction_id.is_empty()
            || !self
                .transaction_id
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            return Err("invalid_outbox_wake_transaction_id".to_owned());
        }
        Ok(())
    }
}

/// Accepted asynchronous continuation invocation.
#[async_trait]
pub trait WakeInvoker: Send + Sync {
    /// Asynchronously invokes this worker with the same durable anchor.
    async fn invoke(&self, wake: &OutboxWake) -> Result<(), String>;
}

/// Delay between bounded transaction-status probes.
#[async_trait]
pub trait WakeDelay: Send + Sync {
    /// Waits without blocking the Lambda runtime thread.
    async fn pause(&self, delay: StdDuration);
}

/// Production asynchronous delay.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioWakeDelay;

#[async_trait]
impl WakeDelay for TokioWakeDelay {
    async fn pause(&self, delay: StdDuration) {
        tokio::time::sleep(delay).await;
    }
}

/// Total pre-commit wait is bounded at 375 ms before Lambda owns the retry.
const PRECOMMIT_RETRY_DELAYS: [StdDuration; 4] = [
    StdDuration::from_millis(25),
    StdDuration::from_millis(50),
    StdDuration::from_millis(100),
    StdDuration::from_millis(200),
];

/// Production continuation adapter, bound to the worker's live alias ARN.
#[derive(Debug, Clone)]
pub struct LambdaWakeInvoker {
    client: aws_sdk_lambda::Client,
    function_arn: String,
}

impl LambdaWakeInvoker {
    /// Binds continuation calls to the exact deployed alias.
    #[must_use]
    pub fn new(client: aws_sdk_lambda::Client, function_arn: String) -> Self {
        Self {
            client,
            function_arn,
        }
    }
}

#[async_trait]
impl WakeInvoker for LambdaWakeInvoker {
    async fn invoke(&self, wake: &OutboxWake) -> Result<(), String> {
        let payload = serde_json::to_vec(wake).map_err(|_| "wake_encode_failed".to_owned())?;
        let response = self
            .client
            .invoke()
            .function_name(&self.function_arn)
            .invocation_type(aws_sdk_lambda::types::InvocationType::Event)
            .payload(aws_smithy_types::Blob::new(payload))
            .send()
            .await
            .map_err(|_| "wake_continuation_rejected".to_owned())?;
        if response.status_code() == 202 {
            Ok(())
        } else {
            Err("wake_continuation_rejected".to_owned())
        }
    }
}

/// The regional authorization projection this worker publishes into.
///
/// A port rather than the concrete [`ProjectionWriter`] for one reason: the
/// **order** of a provision's writes is a contract, and an order is only
/// assertable if a test can record it. Placement publication precedes the
/// central activation commit. `tests/composition.rs` pins that sequence.
#[async_trait]
pub trait RegionalProjection: Send + Sync {
    /// Writes the admission placement row, which is what makes a workspace
    /// visible to the regional edge and is therefore published before the
    /// central activation commit.
    ///
    /// # Errors
    ///
    /// Returns a stable adapter error when the projection cannot be written.
    async fn put_placement(&self, write: &PlacementWrite) -> Result<(), String>;

    /// Writes one key's authorization row, on creation and again on revocation.
    ///
    /// # Errors
    ///
    /// Returns a stable adapter error when the projection cannot be written.
    async fn put_key_authorization(&self, write: &KeyAuthorizationWrite) -> Result<(), String>;
}

#[async_trait]
impl RegionalProjection for ProjectionWriter {
    async fn put_placement(&self, write: &PlacementWrite) -> Result<(), String> {
        Self::put_placement(self, write).await
    }

    async fn put_key_authorization(&self, write: &KeyAuthorizationWrite) -> Result<(), String> {
        Self::put_key_authorization(self, write).await
    }
}

/// The drain itself: one bounded unit of durable control-plane work.
pub struct Worker {
    store: Arc<dyn Store>,
    wake_invoker: Arc<dyn WakeInvoker>,
    wake_delay: Arc<dyn WakeDelay>,
    projection: Arc<dyn RegionalProjection>,
    region: Region,
    clock: Arc<dyn Clock>,
    owner: String,
    batch: u32,
    lease: Duration,
}

#[derive(Debug, PartialEq, Eq)]
enum DispatchDisposition {
    MarkDispatched,
    Release {
        available_at: OffsetDateTime,
        error: String,
    },
}

/// One bounded drain page, used to decide whether continuation is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainReport {
    /// Outbox rows claimed by this invocation.
    pub claimed: usize,
}

fn dispatch_disposition<F>(result: Result<(), String>, attempts: u32, now: F) -> DispatchDisposition
where
    F: FnOnce() -> OffsetDateTime,
{
    match result {
        Ok(()) => DispatchDisposition::MarkDispatched,
        Err(error) => {
            let backoff = i64::from(2_u32.saturating_pow(attempts.min(8))).min(300);
            DispatchDisposition::Release {
                available_at: now() + Duration::seconds(backoff),
                error,
            }
        }
    }
}

/// The bounded drain shape: one unit of work per invoke, leased and owned.
pub struct DrainSettings {
    /// The AWS region this worker drains.
    pub region: Region,
    /// The stable owner spelling recorded against each lease.
    pub owner: String,
    /// The maximum rows one drain admits.
    pub batch: u32,
    /// How long one admitted batch holds its lease.
    pub lease: Duration,
}

impl Worker {
    /// Composes the drain over its authorities.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        wake_invoker: Arc<dyn WakeInvoker>,
        wake_delay: Arc<dyn WakeDelay>,
        projection: Arc<dyn RegionalProjection>,
        settings: DrainSettings,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            wake_invoker,
            wake_delay,
            projection,
            region: settings.region,
            clock,
            owner: settings.owner,
            batch: settings.batch,
            lease: settings.lease,
        }
    }

    /// Drains a bounded unit of durable work. Aurora claims remain the
    /// authority across duplicate asynchronous deliveries.
    ///
    /// # Errors
    ///
    /// Returns a redacted store failure, `operation_recovery_failed` when at
    /// least one claimed operation did not advance, or
    /// `outbox_dispatch_failed` after releasing a failed effect for retry.
    pub async fn tick(&self) -> Result<DrainReport, String> {
        let now = self.clock.now();
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
        let claimed = messages.len();
        let mut dispatch_failed = false;
        for message in messages {
            match dispatch_disposition(self.dispatch(&message).await, message.attempts, || {
                self.clock.now()
            }) {
                DispatchDisposition::MarkDispatched => self
                    .store
                    .mark_outbox_dispatched(message.id, self.clock.now())
                    .await
                    .map_err(redacted_store)?,
                DispatchDisposition::Release {
                    available_at,
                    error,
                } => {
                    dispatch_failed = true;
                    self.store
                        .release_outbox(message.id, available_at, &error)
                        .await
                        .map_err(redacted_store)?;
                }
            }
        }
        if dispatch_failed {
            Err("outbox_dispatch_failed".to_owned())
        } else {
            Ok(DrainReport { claimed })
        }
    }

    /// Resolves a transaction-scoped wake, drains one bounded page, and either
    /// completes the anchor or accepts an asynchronous continuation.
    ///
    /// # Errors
    ///
    /// Identical to [`Worker::tick`], plus the sweep's own store failure.
    pub async fn wake(&self, wake: &OutboxWake) -> Result<(), String> {
        wake.validate()?;
        let mut before = self
            .store
            .outbox_wake_state(wake.anchor_id, &wake.transaction_id)
            .await
            .map_err(redacted_store)?;
        for delay in PRECOMMIT_RETRY_DELAYS {
            if before.transaction != Transaction::InProgress {
                break;
            }
            self.wake_delay.pause(delay).await;
            before = self
                .store
                .outbox_wake_state(wake.anchor_id, &wake.transaction_id)
                .await
                .map_err(redacted_store)?;
        }
        match before.transaction {
            Transaction::Aborted => return Ok(()),
            Transaction::InProgress => return Err("outbox_wake_transaction_in_progress".to_owned()),
            Transaction::Unknown => return Err("outbox_wake_transaction_unknown".to_owned()),
            Transaction::Committed => {}
        }
        if !before.anchor_exists {
            return Err("outbox_wake_anchor_missing".to_owned());
        }

        // Even an already-dispatched anchor drains a page. A statement may
        // insert more than one page (finance fans out to every workspace), and
        // its one continuation necessarily carries the same exact anchor.
        let report = self.tick().await?;
        let after = self
            .store
            .outbox_wake_state(wake.anchor_id, &wake.transaction_id)
            .await
            .map_err(redacted_store)?;
        if report.claimed == usize::try_from(self.batch).unwrap_or(usize::MAX) {
            self.wake_invoker.invoke(wake).await?;
            return Ok(());
        }
        if !after.anchor_dispatched {
            return Err("outbox_wake_anchor_pending".to_owned());
        }

        // No timer owns maintenance now. Sweep only after the durable anchor
        // completed; failures are observable and retried by Lambda.
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
            Topic::AccountStateChanged => self.account_changed(message).await,
            Topic::ApiKeyCreated => {
                self.key_authorization(message, KeyAuthorizationState::Active)
                    .await
            }
            Topic::AuthorizationEpochChanged => {
                self.key_authorization(message, KeyAuthorizationState::Revoked)
                    .await
            }
            Topic::WorkspaceDeleteRequested
            | Topic::InvitationEmailRequested
            | Topic::AuthorizationSigningKeyPublished => Err("retired_outbox_topic".to_owned()),
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
            let view = self
                .store
                .get_workspace_view(workspace.id)
                .await
                .map_err(redacted_store)?
                .ok_or_else(|| "workspace_projection_not_found".to_owned())?;
            self.project_view_with_status(&view, sequence(message.created_at), "active")
                .await?;
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
            return Ok(());
        }
        self.project_workspace(payload.workspace_id, sequence(message.created_at))
            .await
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
            || view.account.revision() < payload.account_revision
        {
            return Err("account_state_payload_mismatch".to_owned());
        }
        self.project_view(&view, payload.changed_at_ms).await
    }

    /// Projects a key's authorization row, on creation and again on revocation.
    ///
    /// Both events carry the whole row rather than a delta, because the two are
    /// dispatched independently and a delta would need the region to already
    /// hold the half it is being told to amend.
    ///
    /// # Why the material is read rather than carried
    ///
    /// The announcement names the key; the verifier, its pepper version and the
    /// effective scopes are read from the authority here. Putting them in the
    /// outbox payload would copy a credential verifier into a queue, a dead
    /// letter and every retry of both, and would let a delayed message publish
    /// material the authority has since re-peppered.
    ///
    /// A key the authority no longer holds publishes **nothing**: there is no
    /// arm that writes a row without a verifier, because a regional edge that
    /// read one would refuse every request against a live key.
    async fn key_authorization(
        &self,
        message: &OutboxMessage,
        state: KeyAuthorizationState,
    ) -> Result<(), String> {
        let payload: KeyAuthorizationPayload = serde_json::from_value(message.payload.clone())
            .map_err(|_| "invalid_key_authorization_payload".to_owned())?;
        if payload.region != self.region {
            return Err("outbox_region_mismatch".to_owned());
        }
        let material = self
            .store
            .workspace_key_material(payload.api_key_id)
            .await
            .map_err(redacted_store)?
            .ok_or_else(|| "api_key_not_found".to_owned())?;
        // The announcement's workspace selected which region's projection is
        // being written. A row naming another workspace would publish a key into
        // a region that must never serve it, so the two authorities are compared
        // rather than one of them being trusted.
        if material.workspace_id != payload.workspace_id {
            return Err("key_authorization_payload_mismatch".to_owned());
        }
        self.projection
            .put_key_authorization(&KeyAuthorizationWrite {
                api_key: api_key(payload.api_key_id)?,
                workspace: workspace(payload.workspace_id)?,
                organization: organization(payload.organization_id)?,
                region: payload.region,
                state,
                verifier: material.verifier,
                pepper_version: material.pepper_version,
                // Every customer-presentable regional audience is explicit on
                // the row, so narrowing reach later remains a control-plane
                // decision rather than a regional-code change.
                //
                // `CUSTOMER_PRESENTABLE` rather than `ALL`, and the one bit
                // between them is the tool executor. A workspace key is a
                // credential a customer presents; the executor's envelope is
                // minted inside an activation and names no presented credential
                // at all, so a key row admitting that audience would be claiming
                // a principal kind a key can never be.
                audiences: AudienceSet::CUSTOMER_PRESENTABLE,
                // The mintable ceiling is applied at creation and again here, so
                // a key whose stored row somehow exceeded it is projected without
                // the excess rather than with it.
                scopes: material
                    .scopes
                    .intersect(ScopeSet::WORKSPACE_KEY_MINTABLE)
                    .to_wire(),
                key_epoch: payload.epoch,
                projection_sequence: sequence(message.created_at),
                updated_at: payload.changed_at,
            })
            .await
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
                if view.account.state()
                    == aex_control_domain::AccountState::PausedTopUpRequired =>
            {
                "paused"
            }
            WorkspaceStatus::Active => "active",
        };
        self.project_view_with_status(view, feed_sequence, status)
            .await
    }

    async fn project_view_with_status(
        &self,
        view: &aex_control_app::ports::WorkspaceView,
        feed_sequence: u64,
        status: &str,
    ) -> Result<(), String> {
        if view.workspace.region != self.region {
            return Err("workspace_region_mismatch".to_owned());
        }
        self.projection
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
            .await?;
        Ok(())
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
struct KeyAuthorizationPayload {
    api_key_id: Uuid,
    workspace_id: Uuid,
    organization_id: Uuid,
    region: Region,
    changed_at: Timestamp,
    epoch: u64,
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
        // One provision operation emits exactly one completion audit. Reusing
        // that durable operation identity makes retries deterministic while
        // keeping every workspace's audit primary key distinct; the nil UUID
        // made the second workspace on a plane conflict forever.
        id: workspace.provision_operation_id,
        organization_id: Some(workspace.organization_id),
        workspace_id: Some(workspace.id),
        actor_kind: ActorKind::System,
        actor_id: None,
        action: action.to_owned(),
        resource_kind: ResourceKind::Workspace,
        resource_id: Some(workspace.id),
        outcome: AuditOutcome::Allowed,
        request_id: RequestId::new("control-projection-worker")
            .as_str()
            .to_owned(),
        operation_id: Some(workspace.provision_operation_id),
        detail: serde_json::json!({ "to_status": "active" }),
        occurred_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::{DispatchDisposition, dispatch_disposition};
    use time::{Duration, OffsetDateTime};

    #[test]
    fn a_projection_failure_is_released_with_an_observable_bounded_retry() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            dispatch_disposition(Err("projection_unavailable".to_owned()), 3, || now),
            DispatchDisposition::Release {
                available_at: now + Duration::seconds(8),
                error: "projection_unavailable".to_owned(),
            }
        );
    }

    #[test]
    fn an_accepted_delivery_is_the_only_outcome_marked_dispatched() {
        assert_eq!(
            dispatch_disposition(Ok(()), 1, || panic!("success needs no retry clock")),
            DispatchDisposition::MarkDispatched
        );
    }
}
