//! Fenced regional workspace authority for central direct invokes.
//!
//! A workspace identifier is preassigned centrally, but its empty regional
//! half is durable here. The row is a tombstone after deletion: identifiers are
//! never resurrected, and an absent delete writes a tombstone so a delayed
//! provision cannot arrive afterwards and create the workspace.

use aex_internal_contracts::control::{
    RegionalControlEnvelope, RegionalControlOutcome, RegionalControlRequest, RegionalRefusal,
};
use aex_internal_contracts::{RunId, SchemaVersion};
use aex_session_app::plan::RootStopReason;
use aex_session_domain::{CancelCause, SessionRevision, cancel_session_fence};
use aex_wire::ids::{AgentId, OrganizationId, SessionId, WorkspaceId};
use aex_wire::types::{Region, Timestamp};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;

use crate::attr::{Item, ItemBuilder, Row, boolean, n, s, stamp};
use crate::plan::{Participant, TransactionPlan, key};

/// The stored discriminator for one regional workspace authority row.
pub const REGIONAL_WORKSPACE_CONTROL: &str = "regional_workspace_control";
const STATES: &[&str] = &["active", "deleted"];
const MAX_CAS_ATTEMPTS: u32 = 3;
const PAUSE_PAGE_SIZE: i32 = 8;

/// `CONTROL#<workspace>` / `WORKSPACE`.
#[must_use]
pub fn workspace_key(workspace: WorkspaceId) -> (String, String) {
    (format!("CONTROL#{workspace}"), "WORKSPACE".to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Deleted,
}

impl State {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceRow {
    workspace: WorkspaceId,
    organization: Option<OrganizationId>,
    region: Region,
    state: State,
    fence: u64,
    intent_hash: Option<String>,
    revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PauseProgress {
    cursor_sk: Option<String>,
    complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveLocator {
    workspace: WorkspaceId,
    organization: OrganizationId,
    session: SessionId,
    run: RunId,
    root: AgentId,
    pause_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RootControl {
    revision: u64,
    cancellation: u64,
    stop_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    Answer(RegionalControlOutcome),
    Write {
        row: WorkspaceRow,
        outcome: RegionalControlOutcome,
    },
}

/// DynamoDB-backed regional workspace control authority.
#[derive(Debug, Clone)]
pub struct RegionalWorkspaceStore {
    client: Client,
    table: String,
    authz_table: String,
    region: Region,
}

impl RegionalWorkspaceStore {
    /// Binds the authority to exactly one regional table and region.
    #[must_use]
    pub fn new(
        client: Client,
        table: impl Into<String>,
        authz_table: impl Into<String>,
        region: Region,
    ) -> Self {
        Self {
            client,
            table: table.into(),
            authz_table: authz_table.into(),
            region,
        }
    }

    /// Performs a strongly consistent read used by startup readiness.
    ///
    /// # Errors
    ///
    /// Returns the AWS diagnostic when the table cannot be read.
    pub async fn probe(&self) -> Result<(), String> {
        self.client
            .get_item()
            .table_name(&self.table)
            .key("pk", s("CONTROL#PROBE"))
            .key("sk", s("WORKSPACE"))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        self.client
            .get_item()
            .table_name(&self.authz_table)
            .key("pk", s("WS#PROBE"))
            .key("sk", s("PLACEMENT"))
            .consistent_read(true)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Applies one versioned request and echoes its request identity.
    pub async fn handle(
        &self,
        request: RegionalControlEnvelope<RegionalControlRequest>,
    ) -> RegionalControlEnvelope<RegionalControlOutcome> {
        let outcome = if request.schema_version == SchemaVersion::V1 {
            Box::pin(self.apply(&request.payload)).await
        } else {
            refused(RegionalRefusal::Internal)
        };
        RegionalControlEnvelope {
            schema_version: SchemaVersion::V1,
            request_id: request.request_id,
            payload: outcome,
        }
    }

    async fn apply(&self, request: &RegionalControlRequest) -> RegionalControlOutcome {
        if request_region(request) != self.region {
            return refused(RegionalRefusal::RegionUnavailable);
        }
        if let RegionalControlRequest::ApplyAccountPause {
            workspace,
            organization,
            account_epoch,
            ..
        } = request
        {
            return Box::pin(self.apply_account_pause(*workspace, *organization, *account_epoch))
                .await;
        }
        let workspace = request_workspace(request);
        for _ in 0..MAX_CAS_ATTEMPTS {
            let Ok(current) = self.read(workspace).await else {
                return refused(RegionalRefusal::Internal);
            };
            match decide(current.as_ref(), request) {
                Decision::Answer(outcome) => return outcome,
                Decision::Write { row, outcome } => {
                    match self.compare_and_set(current.as_ref(), &row).await {
                        Ok(true) => return outcome,
                        Ok(false) => {}
                        Err(()) => {
                            // Resolve an ambiguous write by reading the target. An
                            // identical durable row is the answer, never a reason
                            // to issue a second write under a fresh identity.
                            if self.read(workspace).await.ok().flatten().as_ref() == Some(&row) {
                                return outcome;
                            }
                            return refused(RegionalRefusal::Internal);
                        }
                    }
                }
            }
        }
        refused(RegionalRefusal::RegionUnavailable)
    }

    async fn read(&self, workspace: WorkspaceId) -> Result<Option<WorkspaceRow>, ()> {
        let (pk, sk) = workspace_key(workspace);
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| ())?;
        output
            .item
            .as_ref()
            .map(|item| decode(item, workspace).map_err(|_| ()))
            .transpose()
    }

    async fn compare_and_set(
        &self,
        current: Option<&WorkspaceRow>,
        desired: &WorkspaceRow,
    ) -> Result<bool, ()> {
        let mut put = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(encode(desired)))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld);
        put = match current {
            None => put
                .condition_expression("attribute_not_exists(#pk)")
                .expression_attribute_names("#pk", "pk"),
            Some(current) => put
                .condition_expression("#revision = :revision")
                .expression_attribute_names("#revision", "revision")
                .expression_attribute_values(":revision", n(current.revision)),
        };
        match put.send().await {
            Ok(_) => Ok(true),
            Err(error)
                if error.as_service_error().is_some_and(|service| {
                    matches!(
                        service,
                        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
                    )
                }) => Ok(false),
            Err(_) => Err(()),
        }
    }

    async fn apply_account_pause(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        account_epoch: u64,
    ) -> RegionalControlOutcome {
        match self
            .pause_is_current(workspace, organization, account_epoch)
            .await
        {
            Ok(true) => {}
            // A later resume or pause superseded this exact event. Completing
            // it is safe because every per-session transaction repeats the
            // exact placement predicate.
            Ok(false) => return pause_answer(workspace, true, 0),
            Err(RegionalRefusal::OrganizationMismatch) => {
                return refused(RegionalRefusal::OrganizationMismatch);
            }
            Err(_) => return refused(RegionalRefusal::Internal),
        }
        let progress = match self
            .read_pause_progress(workspace, organization, account_epoch)
            .await
        {
            Ok(progress) => progress,
            Err(reason) => return refused(reason),
        };
        if progress.complete {
            return pause_answer(workspace, true, 0);
        }
        let (locators, next) = match self
            .query_active_sessions(workspace, organization, progress.cursor_sk.as_deref())
            .await
        {
            Ok(page) => page,
            Err(reason) => return refused(reason),
        };
        let mut interrupted = 0_u32;
        for locator in &locators {
            if locator.pause_epoch >= account_epoch {
                continue;
            }
            match self.interrupt_active_session(locator, account_epoch).await {
                Ok(true) => interrupted = interrupted.saturating_add(1),
                Ok(false) => {}
                Err(RegionalRefusal::FenceSuperseded) => {
                    return pause_answer(workspace, true, interrupted);
                }
                Err(reason) => return refused(reason),
            }
        }
        let complete = next.is_none();
        if self
            .write_pause_progress(
                workspace,
                organization,
                account_epoch,
                next.as_deref(),
                complete,
            )
            .await
            .is_err()
        {
            return refused(RegionalRefusal::Internal);
        }
        pause_answer(workspace, complete, interrupted)
    }

    async fn pause_is_current(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        account_epoch: u64,
    ) -> Result<bool, RegionalRefusal> {
        let physical = crate::keys::authorization_placement(workspace);
        let output = self
            .client
            .get_item()
            .table_name(&self.authz_table)
            .key("pk", s(physical.pk))
            .key("sk", s(physical.sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| RegionalRefusal::Internal)?;
        let Some(item) = output.item else {
            return Err(RegionalRefusal::Internal);
        };
        let row = Row::bind(&item, "workspace_placement").map_err(|_| RegionalRefusal::Internal)?;
        if row
            .id::<OrganizationId>("organizationId")
            .map_err(|_| RegionalRefusal::Internal)?
            != organization
        {
            return Err(RegionalRefusal::OrganizationMismatch);
        }
        let stored_epoch = row
            .u64("accountEpoch")
            .map_err(|_| RegionalRefusal::Internal)?;
        let status = row
            .string("status")
            .map_err(|_| RegionalRefusal::Internal)?;
        if stored_epoch < account_epoch {
            return Err(RegionalRefusal::Internal);
        }
        Ok(stored_epoch == account_epoch && status == "paused")
    }

    async fn read_pause_progress(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        account_epoch: u64,
    ) -> Result<PauseProgress, RegionalRefusal> {
        let (pk, sk) = pause_progress_key(workspace, account_epoch);
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| RegionalRefusal::Internal)?;
        let Some(item) = output.item else {
            return Ok(PauseProgress {
                cursor_sk: None,
                complete: false,
            });
        };
        let row = Row::bind(&item, crate::codec::ACCOUNT_PAUSE_PROGRESS)
            .map_err(|_| RegionalRefusal::Internal)?;
        row.owned_by("organizationId", &organization.to_string())
            .map_err(|_| RegionalRefusal::OrganizationMismatch)?;
        row.owned_by("workspaceId", &workspace.to_string())
            .map_err(|_| RegionalRefusal::OrganizationMismatch)?;
        if row
            .u64("accountEpoch")
            .map_err(|_| RegionalRefusal::Internal)?
            != account_epoch
        {
            return Err(RegionalRefusal::Internal);
        }
        Ok(PauseProgress {
            cursor_sk: row
                .opt_string("cursorSk")
                .map_err(|_| RegionalRefusal::Internal)?
                .map(str::to_owned),
            complete: row
                .boolean("complete")
                .map_err(|_| RegionalRefusal::Internal)?,
        })
    }

    async fn query_active_sessions(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        cursor_sk: Option<&str>,
    ) -> Result<(Vec<ActiveLocator>, Option<String>), RegionalRefusal> {
        let pk = format!("ACTIVE#{workspace}");
        let mut query = self
            .client
            .query()
            .table_name(&self.table)
            .consistent_read(true)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(":pk", s(pk.clone()))
            .limit(PAUSE_PAGE_SIZE);
        if let Some(cursor) = cursor_sk {
            query = query
                .exclusive_start_key("pk", s(pk))
                .exclusive_start_key("sk", s(cursor));
        }
        let output = query.send().await.map_err(|_| RegionalRefusal::Internal)?;
        let mut locators = Vec::with_capacity(output.items().len());
        for item in output.items() {
            let locator = decode_active_locator(item).map_err(|_| RegionalRefusal::Internal)?;
            if locator.workspace != workspace || locator.organization != organization {
                return Err(RegionalRefusal::OrganizationMismatch);
            }
            locators.push(locator);
        }
        let next = output
            .last_evaluated_key
            .as_ref()
            .and_then(|key| key.get("sk"))
            .and_then(|value| value.as_s().ok())
            .cloned();
        Ok((locators, next))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the four participants and their shared exact-epoch fence are one atomic transaction"
    )]
    async fn interrupt_active_session(
        &self,
        locator: &ActiveLocator,
        account_epoch: u64,
    ) -> Result<bool, RegionalRefusal> {
        let session_key = crate::keys::head(locator.session);
        let root_key = crate::keys::agent_control(locator.session, locator.root);
        let session_read = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(session_key.pk.clone()))
            .key("sk", s(session_key.sk.clone()))
            .consistent_read(true)
            .send();
        let root_read = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(root_key.pk.clone()))
            .key("sk", s(root_key.sk.clone()))
            .consistent_read(true)
            .send();
        let (session_output, root_output) =
            futures::try_join!(session_read, root_read).map_err(|_| RegionalRefusal::Internal)?;
        let Some(session_item) = session_output.item else {
            return self.retire_stale_locator(locator).await.map(|()| false);
        };
        let Ok(session) = crate::authority_codec::decode_session(&session_item, locator.workspace)
        else {
            return Err(RegionalRefusal::Internal);
        };
        if session.organization != locator.organization
            || session.active_run != Some(locator.run)
            || session.root_agent != locator.root
        {
            return self.retire_stale_locator(locator).await.map(|()| false);
        }
        let Some(root_item) = root_output.item else {
            return Err(RegionalRefusal::Internal);
        };
        let root = decode_root_control(&root_item, locator)?;
        if root.stop_requested {
            self.mark_locator_applied(locator, account_epoch).await?;
            return Ok(false);
        }
        if root.cancellation != session.cancellation.0 {
            return Err(RegionalRefusal::RegionUnavailable);
        }
        let now = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
            .map_err(|_| RegionalRefusal::Internal)?;
        let fence = cancel_session_fence(&session, CancelCause::AccountPaused, true);
        let mut next_session = session.clone();
        next_session.cancellation = fence.cancellation;
        next_session.work_admission = fence.admission;
        next_session.revision = SessionRevision(session.revision.0.saturating_add(1));
        next_session.updated_at = now;

        let placement = crate::keys::authorization_placement(locator.workspace);
        let active = crate::keys::active_session(locator.workspace, locator.session);
        let mut plan = TransactionPlan::new(format!(
            "pause-{}-{}-{}",
            account_epoch, locator.session, session.revision.0
        ));
        plan.condition_check(
            Participant::AUTHZ_PLACEMENT,
            aws_sdk_dynamodb::types::ConditionCheck::builder()
                .table_name(&self.authz_table)
                .set_key(Some(key(&placement.pk, &placement.sk)))
                .condition_expression(
                    "organizationId = :organization AND accountEpoch = :accountEpoch AND #status = :paused",
                )
                .expression_attribute_names("#status", "status")
                .expression_attribute_values(":organization", s(locator.organization.to_string()))
                .expression_attribute_values(":accountEpoch", n(account_epoch))
                .expression_attribute_values(":paused", s("paused")),
        )
        .map_err(|_| RegionalRefusal::Internal)?;
        plan.put(
            Participant::SESSION_HEAD,
            aws_sdk_dynamodb::types::Put::builder()
                .table_name(&self.table)
                .set_item(Some(
                    crate::authority_codec::encode_session(&next_session)
                        .map_err(|_| RegionalRefusal::Internal)?,
                ))
                .condition_expression(
                    "revision = :revision AND activeRunId = :run AND cancelEpoch = :cancelEpoch",
                )
                .expression_attribute_values(":revision", n(session.revision.0))
                .expression_attribute_values(":run", s(locator.run.to_string()))
                .expression_attribute_values(":cancelEpoch", n(session.cancellation.0)),
        )
        .map_err(|_| RegionalRefusal::Internal)?;
        plan.update(
            Participant::AGENT_ROOT_CONTROL,
            aws_sdk_dynamodb::types::Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&root_key.pk, &root_key.sk)))
                .condition_expression(
                    "revision = :revision AND cancelEpoch = :cancelEpoch AND attribute_not_exists(finishReason)",
                )
                .update_expression(
                    "SET stopRequested = :requested, stopReason = :reason, cancelEpoch = :nextCancelEpoch, revision = :nextRevision, updatedAt = :now",
                )
                .expression_attribute_values(":revision", n(root.revision))
                .expression_attribute_values(":cancelEpoch", n(root.cancellation))
                .expression_attribute_values(":requested", boolean(true))
                .expression_attribute_values(":reason", s(RootStopReason::AccountPaused.as_str()))
                .expression_attribute_values(":nextCancelEpoch", n(fence.cancellation.0))
                .expression_attribute_values(":nextRevision", n(root.revision.saturating_add(1)))
                .expression_attribute_values(":now", stamp(now)),
        )
        .map_err(|_| RegionalRefusal::Internal)?;
        plan.update(
            Participant::SESSION_ACTIVE_LOCATOR,
            aws_sdk_dynamodb::types::Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&active.pk, &active.sk)))
                .condition_expression("runId = :run AND pauseEpoch < :pauseEpoch")
                .update_expression(
                    "SET pauseEpoch = :pauseEpoch, cancelEpoch = :cancelEpoch, updatedAt = :now",
                )
                .expression_attribute_values(":run", s(locator.run.to_string()))
                .expression_attribute_values(":pauseEpoch", n(account_epoch))
                .expression_attribute_values(":cancelEpoch", n(fence.cancellation.0))
                .expression_attribute_values(":now", stamp(now)),
        )
        .map_err(|_| RegionalRefusal::Internal)?;
        let request = plan
            .compile(&self.client)
            .map_err(|_| RegionalRefusal::Internal)?;
        match request.send().await {
            Ok(_) => Ok(true),
            Err(_) => match self
                .pause_is_current(locator.workspace, locator.organization, account_epoch)
                .await
            {
                Ok(false) => Err(RegionalRefusal::FenceSuperseded),
                Ok(true) => Err(RegionalRefusal::RegionUnavailable),
                Err(reason) => Err(reason),
            },
        }
    }

    async fn mark_locator_applied(
        &self,
        locator: &ActiveLocator,
        account_epoch: u64,
    ) -> Result<(), RegionalRefusal> {
        let placement = crate::keys::authorization_placement(locator.workspace);
        let active = crate::keys::active_session(locator.workspace, locator.session);
        let mut plan =
            TransactionPlan::new(format!("pause-mark-{}-{}", account_epoch, locator.session));
        plan.condition_check(
            Participant::AUTHZ_PLACEMENT,
            aws_sdk_dynamodb::types::ConditionCheck::builder()
                .table_name(&self.authz_table)
                .set_key(Some(key(&placement.pk, &placement.sk)))
                .condition_expression(
                    "organizationId = :organization AND accountEpoch = :accountEpoch AND #status = :paused",
                )
                .expression_attribute_names("#status", "status")
                .expression_attribute_values(":organization", s(locator.organization.to_string()))
                .expression_attribute_values(":accountEpoch", n(account_epoch))
                .expression_attribute_values(":paused", s("paused")),
        )
        .map_err(|_| RegionalRefusal::Internal)?;
        plan.update(
            Participant::SESSION_ACTIVE_LOCATOR,
            aws_sdk_dynamodb::types::Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&active.pk, &active.sk)))
                .condition_expression("runId = :run AND pauseEpoch < :pauseEpoch")
                .update_expression("SET pauseEpoch = :pauseEpoch")
                .expression_attribute_values(":run", s(locator.run.to_string()))
                .expression_attribute_values(":pauseEpoch", n(account_epoch)),
        )
        .map_err(|_| RegionalRefusal::Internal)?;
        plan.compile(&self.client)
            .map_err(|_| RegionalRefusal::Internal)?
            .send()
            .await
            .map(|_| ())
            .map_err(|_| RegionalRefusal::RegionUnavailable)
    }

    async fn retire_stale_locator(&self, locator: &ActiveLocator) -> Result<(), RegionalRefusal> {
        let active = crate::keys::active_session(locator.workspace, locator.session);
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", s(active.pk))
            .key("sk", s(active.sk))
            .condition_expression("attribute_not_exists(pk) OR runId = :run")
            .expression_attribute_values(":run", s(locator.run.to_string()))
            .send()
            .await
            .map(|_| ())
            .map_err(|error| {
                if error.as_service_error().is_some_and(|service| {
                    matches!(service, aws_sdk_dynamodb::operation::delete_item::DeleteItemError::ConditionalCheckFailedException(_))
                }) {
                    RegionalRefusal::RegionUnavailable
                } else {
                    RegionalRefusal::Internal
                }
            })
    }

    async fn write_pause_progress(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        account_epoch: u64,
        cursor_sk: Option<&str>,
        complete: bool,
    ) -> Result<(), ()> {
        let (pk, sk) = pause_progress_key(workspace, account_epoch);
        let now =
            Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc()).map_err(|_| ())?;
        let item = ItemBuilder::new(crate::codec::ACCOUNT_PAUSE_PROGRESS)
            .set("pk", s(pk))
            .set("sk", s(sk))
            .set("workspaceId", s(workspace.to_string()))
            .set("organizationId", s(organization.to_string()))
            .set("accountEpoch", n(account_epoch))
            .set_opt("cursorSk", cursor_sk.map(s))
            .set("complete", boolean(complete))
            .set("updatedAt", stamp(now))
            .build();
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .send()
            .await
            .map(|_| ())
            .map_err(|_| ())
    }
}

fn pause_progress_key(workspace: WorkspaceId, account_epoch: u64) -> (String, String) {
    (
        format!("CONTROL#{workspace}"),
        format!("ACCOUNT_PAUSE#{account_epoch:020}"),
    )
}

fn pause_answer(
    workspace: WorkspaceId,
    complete: bool,
    interrupted: u32,
) -> RegionalControlOutcome {
    RegionalControlOutcome::AccountPauseApplied {
        workspace,
        complete,
        interrupted,
    }
}

fn decode_active_locator(item: &Item) -> Result<ActiveLocator, crate::attr::CodecError> {
    let row = Row::bind(item, crate::codec::ACTIVE_SESSION)?;
    let locator = ActiveLocator {
        workspace: row.id("workspaceId")?,
        organization: row.id("organizationId")?,
        session: row.id("sessionId")?,
        run: row.run_id("runId")?,
        root: row.id("rootAgentId")?,
        pause_epoch: row.u64("pauseEpoch")?,
    };
    let physical = crate::keys::active_session(locator.workspace, locator.session);
    for (attribute, expected) in [
        (crate::attr::PK, physical.pk.as_str()),
        (crate::attr::SK, physical.sk.as_str()),
    ] {
        let found = row.string(attribute)?;
        if found != expected {
            return Err(crate::attr::CodecError::Malformed {
                item_type: crate::codec::ACTIVE_SESSION,
                attribute,
                reason: "the physical key does not match the locator identity".to_owned(),
            });
        }
    }
    Ok(locator)
}

fn decode_root_control(
    item: &Item,
    locator: &ActiveLocator,
) -> Result<RootControl, RegionalRefusal> {
    let row =
        Row::bind(item, crate::codec::AGENT_CONTROL).map_err(|_| RegionalRefusal::Internal)?;
    row.owned_by("workspaceId", &locator.workspace.to_string())
        .map_err(|_| RegionalRefusal::OrganizationMismatch)?;
    if row
        .id::<SessionId>("sessionId")
        .map_err(|_| RegionalRefusal::Internal)?
        != locator.session
        || row
            .id::<AgentId>("agentId")
            .map_err(|_| RegionalRefusal::Internal)?
            != locator.root
    {
        return Err(RegionalRefusal::Internal);
    }
    Ok(RootControl {
        revision: row.u64("revision").map_err(|_| RegionalRefusal::Internal)?,
        cancellation: row
            .opt_u64("cancelEpoch")
            .map_err(|_| RegionalRefusal::Internal)?
            .unwrap_or(0),
        stop_requested: row.boolean("stopRequested").unwrap_or(false),
    })
}

fn request_workspace(request: &RegionalControlRequest) -> WorkspaceId {
    match request {
        RegionalControlRequest::ProvisionWorkspace { workspace, .. }
        | RegionalControlRequest::DeleteWorkspace { workspace, .. }
        | RegionalControlRequest::ApplyAccountPause { workspace, .. } => *workspace,
    }
}

fn request_region(request: &RegionalControlRequest) -> Region {
    match request {
        RegionalControlRequest::ProvisionWorkspace { region, .. }
        | RegionalControlRequest::DeleteWorkspace { region, .. }
        | RegionalControlRequest::ApplyAccountPause { region, .. } => *region,
    }
}

fn decide(current: Option<&WorkspaceRow>, request: &RegionalControlRequest) -> Decision {
    match request {
        RegionalControlRequest::ProvisionWorkspace {
            workspace,
            organization,
            region,
            fence,
            intent_hash,
        } => decide_provision(
            current,
            *workspace,
            *organization,
            *region,
            *fence,
            intent_hash,
        ),
        RegionalControlRequest::DeleteWorkspace {
            workspace,
            region,
            fence,
        } => decide_delete(current, *workspace, *region, *fence),
        RegionalControlRequest::ApplyAccountPause { .. } => {
            Decision::Answer(refused(RegionalRefusal::Internal))
        }
    }
}

fn decide_provision(
    current: Option<&WorkspaceRow>,
    workspace: WorkspaceId,
    organization: OrganizationId,
    region: Region,
    fence: u64,
    intent_hash: &str,
) -> Decision {
    let success = |created| RegionalControlOutcome::WorkspaceProvisioned { workspace, created };
    let Some(current) = current else {
        return Decision::Write {
            row: WorkspaceRow {
                workspace,
                organization: Some(organization),
                region,
                state: State::Active,
                fence,
                intent_hash: Some(intent_hash.to_owned()),
                revision: 1,
            },
            outcome: success(true),
        };
    };
    if current
        .organization
        .is_some_and(|owner| owner != organization)
    {
        return Decision::Answer(refused(RegionalRefusal::OrganizationMismatch));
    }
    if current.fence > fence {
        return Decision::Answer(refused(RegionalRefusal::FenceSuperseded));
    }
    if current.state == State::Deleted || current.intent_hash.as_deref() != Some(intent_hash) {
        return Decision::Answer(refused(RegionalRefusal::IntentConflict));
    }
    if current.fence == fence {
        return Decision::Answer(success(false));
    }
    let mut advanced = current.clone();
    advanced.fence = fence;
    advanced.revision = advanced.revision.saturating_add(1);
    Decision::Write {
        row: advanced,
        outcome: success(false),
    }
}

fn decide_delete(
    current: Option<&WorkspaceRow>,
    workspace: WorkspaceId,
    region: Region,
    fence: u64,
) -> Decision {
    let success = |removed| RegionalControlOutcome::WorkspaceDeleted { workspace, removed };
    let Some(current) = current else {
        return Decision::Write {
            row: WorkspaceRow {
                workspace,
                organization: None,
                region,
                state: State::Deleted,
                fence,
                intent_hash: None,
                revision: 1,
            },
            outcome: success(false),
        };
    };
    if current.fence > fence {
        return Decision::Answer(refused(RegionalRefusal::FenceSuperseded));
    }
    if current.state == State::Deleted && current.fence == fence {
        return Decision::Answer(success(false));
    }
    let removed = current.state == State::Active;
    let mut tombstone = current.clone();
    tombstone.state = State::Deleted;
    tombstone.fence = fence;
    tombstone.revision = tombstone.revision.saturating_add(1);
    Decision::Write {
        row: tombstone,
        outcome: success(removed),
    }
}

fn refused(reason: RegionalRefusal) -> RegionalControlOutcome {
    RegionalControlOutcome::Refused { reason }
}

fn encode(row: &WorkspaceRow) -> Item {
    let (pk, sk) = workspace_key(row.workspace);
    ItemBuilder::new(REGIONAL_WORKSPACE_CONTROL)
        .set("pk", s(pk))
        .set("sk", s(sk))
        .set("workspaceId", s(row.workspace.to_string()))
        .set_opt(
            "organizationId",
            row.organization.map(|id| s(id.to_string())),
        )
        .set("region", s(row.region.as_str()))
        .set("state", s(row.state.as_str()))
        .set("fence", n(row.fence))
        .set_opt(
            "intentHash",
            row.intent_hash.as_ref().map(|hash| s(hash.clone())),
        )
        .set("revision", n(row.revision))
        .build()
}

fn decode(item: &Item, asserted: WorkspaceId) -> Result<WorkspaceRow, crate::attr::CodecError> {
    let row = Row::bind(item, REGIONAL_WORKSPACE_CONTROL)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let state = match row.enumerated("state", STATES)? {
        "active" => State::Active,
        "deleted" => State::Deleted,
        _ => unreachable!("enumerated returned outside its vocabulary"),
    };
    let region_text = row.string("region")?;
    let region =
        Region::from_name(region_text).ok_or_else(|| crate::attr::CodecError::Malformed {
            item_type: REGIONAL_WORKSPACE_CONTROL,
            attribute: "region",
            reason: format!("`{region_text}` is not a launch region"),
        })?;
    Ok(WorkspaceRow {
        workspace: asserted,
        organization: row.opt_id("organizationId")?,
        region,
        state,
        fence: row.u64("fence")?,
        intent_hash: row.opt_string("intentHash")?.map(str::to_owned),
        revision: row.u64("revision")?,
    })
}

#[cfg(test)]
mod tests {
    use super::{Decision, State, WorkspaceRow, decide, decode, decode_active_locator, encode};
    use aex_internal_contracts::RunId;
    use aex_internal_contracts::control::{
        RegionalControlOutcome, RegionalControlRequest, RegionalRefusal,
    };
    use aex_wire::ids::{AgentId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Region;

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn organization(byte: u8) -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1, [byte; 10]))
    }

    fn provision(fence: u64, intent: &str) -> RegionalControlRequest {
        RegionalControlRequest::ProvisionWorkspace {
            workspace: workspace(),
            organization: organization(2),
            region: Region::EuWest1,
            fence,
            intent_hash: intent.to_owned(),
        }
    }

    fn written(decision: Decision) -> WorkspaceRow {
        match decision {
            Decision::Write { row, .. } => row,
            Decision::Answer(answer) => panic!("expected a write, got {answer:?}"),
        }
    }

    #[test]
    fn provision_replay_is_one_workspace_and_a_newer_same_intent_advances_the_fence() {
        let first = written(decide(None, &provision(1, "aa")));
        assert_eq!(first.state, State::Active);
        assert!(matches!(
            decide(Some(&first), &provision(1, "aa")),
            Decision::Answer(RegionalControlOutcome::WorkspaceProvisioned { created: false, .. })
        ));
        let advanced = written(decide(Some(&first), &provision(2, "aa")));
        assert_eq!(advanced.fence, 2);
        assert_eq!(advanced.revision, 2);
    }

    #[test]
    fn intent_tenant_and_stale_fence_conflicts_are_typed() {
        let current = written(decide(None, &provision(3, "aa")));
        for (request, expected) in [
            (provision(2, "aa"), RegionalRefusal::FenceSuperseded),
            (provision(3, "bb"), RegionalRefusal::IntentConflict),
            (
                RegionalControlRequest::ProvisionWorkspace {
                    workspace: workspace(),
                    organization: organization(9),
                    region: Region::EuWest1,
                    fence: 3,
                    intent_hash: "aa".to_owned(),
                },
                RegionalRefusal::OrganizationMismatch,
            ),
        ] {
            assert!(matches!(
                decide(Some(&current), &request),
                Decision::Answer(RegionalControlOutcome::Refused { reason }) if reason == expected
            ));
        }
    }

    #[test]
    fn deletion_is_idempotent_and_tombstones_an_absent_workspace() {
        let delete = RegionalControlRequest::DeleteWorkspace {
            workspace: workspace(),
            region: Region::EuWest1,
            fence: 4,
        };
        let absent = written(decide(None, &delete));
        assert_eq!(absent.state, State::Deleted);
        assert!(matches!(
            decide(Some(&absent), &delete),
            Decision::Answer(RegionalControlOutcome::WorkspaceDeleted { removed: false, .. })
        ));
        assert!(matches!(
            decide(Some(&absent), &provision(5, "aa")),
            Decision::Answer(RegionalControlOutcome::Refused {
                reason: RegionalRefusal::IntentConflict
            })
        ));
    }

    #[test]
    fn the_authority_row_codec_is_strict_and_lossless() {
        let row = written(decide(None, &provision(7, "deadbeef")));
        assert_eq!(decode(&encode(&row), workspace()).expect("decodes"), row);
        let wrong = WorkspaceId::from_uuid7(Uuid7::compose(1, [9; 10]));
        assert!(decode(&encode(&row), wrong).is_err());
    }

    #[test]
    fn an_active_locator_must_agree_with_its_workspace_scoped_physical_key() {
        let session = SessionId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let physical = crate::keys::active_session(workspace(), session);
        let row = crate::attr::ItemBuilder::new(crate::codec::ACTIVE_SESSION)
            .set(crate::attr::PK, crate::attr::s(physical.pk))
            .set(crate::attr::SK, crate::attr::s(physical.sk))
            .set("workspaceId", crate::attr::s(workspace().to_string()))
            .set(
                "organizationId",
                crate::attr::s(organization(2).to_string()),
            )
            .set("sessionId", crate::attr::s(session.to_string()))
            .set(
                "runId",
                crate::attr::s(RunId::from_uuid7(Uuid7::compose(1, [4; 10])).to_string()),
            )
            .set(
                "rootAgentId",
                crate::attr::s(AgentId::from_uuid7(Uuid7::compose(1, [5; 10])).to_string()),
            )
            .set("pauseEpoch", crate::attr::n(0))
            .build();
        assert!(decode_active_locator(&row).is_ok());

        let mut forged = row;
        forged.insert(crate::attr::SK.to_owned(), crate::attr::s("SESSION#forged"));
        assert!(decode_active_locator(&forged).is_err());
    }
}
