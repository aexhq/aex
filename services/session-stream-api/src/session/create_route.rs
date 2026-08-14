//! Production session admission and background sandbox preparation.
//!
//! A receipt-keyed private preparation elects every identity and selected file
//! revision. Root `AgentStarted`, the requested generation row, public head and
//! exact response receipt become durable before this module starts provider or
//! guest I/O. A best-effort eager task starts the elected preparation; Tool Mux
//! recovers the same manifest from detached first-call polling and uses the
//! same exact-generation runtime authority.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aex_brain_domain::budget::Dimension;
use aex_brain_domain::journal::JournalRecord;
use aex_hands_protocol::files::{FileRequest, FileResponse};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_session_app::ports::{
    AgentExecutionLimits, IdFactory as _, RunBudgetLimits, SessionReader as _,
};
use aex_session_app::{
    CreateSession, PrepareSessionCreateOutcome, PreparedSessionCreate, RequestedSessionLaunch,
    ResolvedInitialFile, RootStartedEvidence, initial_root_record, prepare_session_create,
    publish_requested_session, replay_session_create_receipt,
};
use aex_session_domain::{
    AccountRevision, AgentRevision, IdempotencyIdentity, JournalSeq, PinnedRuntime,
    ProviderCredentialPin, ReceiptKey, ResolvedConfigAuthority, SessionMetadata,
};
use aex_session_dynamodb::app_authority::{ApiHintSink, DynamoAuthorityCommitter};
use aex_session_dynamodb::application_plan::{FamilyCompilers, SessionBinding};
use aex_session_dynamodb::create_preparation::{
    CreatePreparation, CreatePreparationError, CreatePreparationStore, DurableRootStarted,
    PreparedFile, RootStarted,
};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::Participant;
use aex_session_dynamodb::sandbox_preparation::{
    SANDBOX_PREPARATION_HEARTBEAT_MILLIS, SANDBOX_PREPARATION_LEASE_MILLIS,
    SandboxPreparationAuthority, SandboxPreparationAuthorityError, SandboxPreparationClaim,
    SandboxPreparationLease,
};
use aex_wire::canonical::{CanonicalJson, to_jcs_bytes};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::{IntentDigest, ReplayIdentity};
use aex_wire::models;
use aex_wire::routes::RouteId;
use aex_wire::types::{DecimalU128, Timestamp};

use super::create_readiness::{
    ContentObjects, StartupBody, StartupContent, StartupFileError, StartupGuest, StartupGuestError,
    materialize_selected,
};
use super::handlers::{Routes, Shared};

/// Runs the public admission path to a truthful requested `201`.
pub(super) async fn create_session(
    routes: &Routes,
    request: models::SessionCreateRequest,
) -> WireResult<models::Session> {
    let command = command(routes, request)?;
    let receipt_key = ReceiptKey::of(aex_session_app::CREATE_SCOPE, &command.identity)
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let bindings = routes.bindings()?;
    let now = routes.now()?;

    if let Some(session) = replay_existing(routes, &command, now).await? {
        return Ok(session);
    }

    let preparations = CreatePreparationStore::new(
        routes.shared.authority.clone(),
        routes.shared.tables.session_authority.clone(),
    );
    let prepared = if let Some(winner) = preparations
        .load(command.workspace, receipt_key.key_sha256())
        .await
        .map_err(|error| preparation_failure(&error))?
    {
        require_winner_intent(&winner, &command)?;
        rehydrate(&winner, &command)?
    } else {
        let context = bindings.context();
        match prepare_session_create(&context, &command)
            .await
            .map_err(|error| create_app_failure(&error))?
        {
            PrepareSessionCreateOutcome::Replayed { session, .. } => return Ok(*session),
            PrepareSessionCreateOutcome::Prepared(candidate) => {
                let candidate = private_preparation(
                    candidate.as_ref(),
                    receipt_key.key_sha256(),
                    *bindings.ids(),
                )?;
                let winner = preparations
                    .stage_and_elect(&candidate)
                    .await
                    .map_err(|error| preparation_failure(&error))?;
                require_winner_intent(&winner, &command)?;
                rehydrate(&winner, &command)?
            }
        }
    };
    let winner = preparations
        .load(command.workspace, receipt_key.key_sha256())
        .await
        .map_err(|error| preparation_failure(&error))?
        .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
    derive_runtime_authority(routes, &prepared, &winner).await?;
    let launch = establish_admission(routes, &preparations, &winner).await?;
    publish_requested_head(routes, &command, &prepared, &winner, &launch).await
}

async fn replay_existing(
    routes: &Routes,
    command: &CreateSession,
    now: Timestamp,
) -> WireResult<Option<models::Session>> {
    // Receipt before both private election and every volatile planning read.
    // An accepted retry remains independent of a later pause, credential
    // revocation, registry deletion or model-catalog change.
    let Some(stored) = routes
        .shared
        .commands
        .load_receipt(
            command.workspace,
            aex_session_app::CREATE_SCOPE,
            &command.identity,
            now,
        )
        .await
        .map_err(port_failure)?
    else {
        return Ok(None);
    };
    let session = replayed(
        replay_session_create_receipt(&stored, command)
            .map_err(|error| create_app_failure(&error))?,
    )?;
    if session.sandbox_status == models::SandboxStatus::Requested {
        schedule_replayed_eager_preparation(routes.shared.clone(), command.workspace, session.id);
    }
    Ok(Some(session))
}

async fn derive_runtime_authority(
    routes: &Routes,
    prepared: &PreparedSessionCreate,
    winner: &CreatePreparation,
) -> WireResult<()> {
    let Some(definition) = prepared
        .pinned_runtime
        .as_ref()
        .map(PinnedRuntime::definition)
    else {
        debug_assert!(winner.generation.is_none());
        return Ok(());
    };
    if let Err(error) = aex_runtime_activity_dynamodb::derive::derive_generation_rows(
        &routes.shared.runtime_activity,
        definition,
        winner.prepared_at,
    )
    .await
    {
        eprintln!("session-stream-api: session create could not derive runtime authority: {error}");
        return Err(WireError::new(ErrorCode::InternalError));
    }
    Ok(())
}

async fn establish_admission(
    routes: &Routes,
    preparations: &CreatePreparationStore,
    winner: &CreatePreparation,
) -> WireResult<RequestedSessionLaunch> {
    let root = persist_root_started(routes, preparations, winner, winner.prepared_at).await?;
    Ok(RequestedSessionLaunch {
        root_started: RootStartedEvidence {
            agent: winner.root_agent,
            session: winner.session,
            generation: winner.generation,
            occurred_at: root.occurred_at,
            revision: AgentRevision(root.revision),
            journal_tail: JournalSeq(root.journal_tail),
            journal_tail_hash: root.journal_tail_hash,
        },
    })
}

async fn persist_root_started(
    routes: &Routes,
    preparations: &CreatePreparationStore,
    winner: &CreatePreparation,
    occurred_at: Timestamp,
) -> WireResult<DurableRootStarted> {
    match preparations
        .commit_root_started(winner, RootStarted { occurred_at })
        .await
    {
        Ok(root) => Ok(root),
        Err(error) => {
            compensate(routes, winner, "root AgentStarted", &error).await;
            Err(preparation_failure(&error))
        }
    }
}

async fn publish_requested_head(
    routes: &Routes,
    command: &CreateSession,
    prepared: &PreparedSessionCreate,
    winner: &CreatePreparation,
    launch: &RequestedSessionLaunch,
) -> WireResult<models::Session> {
    let planned = match publish_requested_session(prepared, launch) {
        Ok(planned) => planned,
        Err(error) => {
            compensate(routes, winner, "requested publication planning", &error).await;
            return Err(create_app_failure(&error));
        }
    };
    let binding = SessionBinding {
        workspace: winner.workspace,
        organization: winner.organization,
        session: winner.session,
    };
    let committer = DynamoAuthorityCommitter::new(
        routes.shared.authority.clone(),
        routes.shared.tables.clone(),
        binding,
        launch.root_started.occurred_at,
        ApiHintSink,
    );
    if let Err(error) = committer
        .commit_replayable(&planned.plan, &FamilyCompilers::new())
        .await
    {
        let response = recover_publication(
            routes,
            command,
            winner,
            launch.root_started.occurred_at,
            &error,
        )
        .await?;
        schedule_eager_preparation(routes.shared.clone(), winner.clone());
        return Ok(response);
    }
    let response = aex_session_app::public_session(&planned.projected)
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    schedule_eager_preparation(routes.shared.clone(), winner.clone());
    Ok(response)
}

/// Starts the eager optimization only after Requested is publicly durable.
///
/// This process-local task is deliberately not the correctness mechanism: a
/// task or service instance may stop at any await. Tool Mux reloads the same
/// elected preparation by session on the first detached tool poll and
/// idempotently completes any missing step.
fn schedule_eager_preparation(shared: std::sync::Arc<Shared>, prepared: CreatePreparation) {
    if prepared.generation.is_none() {
        return;
    }
    tokio::spawn(async move {
        if let Err(reason) = eagerly_prepare_sandbox(&shared, &prepared).await {
            tracing::warn!(
                target: "aex::session_telemetry",
                event = "sandbox_progress_failed",
                progress = reason,
                session = %prepared.session,
                generation = %prepared.generation.expect("checked before spawn"),
            );
        }
    });
}

fn schedule_replayed_eager_preparation(
    shared: std::sync::Arc<Shared>,
    workspace: aex_wire::ids::WorkspaceId,
    session: aex_wire::ids::SessionId,
) {
    tokio::spawn(async move {
        let result = async {
            let head = shared
                .commands
                .load_session_strong(workspace, session)
                .await
                .map_err(|_| "loading_replayed_session")?;
            let preparations = CreatePreparationStore::new(
                shared.authority.clone(),
                shared.tables.session_authority.clone(),
            );
            let prepared = preparations
                .load_for_session(workspace, session, head.generation)
                .await
                .map_err(|_| "loading_replayed_preparation")?
                .ok_or("loading_replayed_preparation")?;
            eagerly_prepare_sandbox(&shared, &prepared).await
        }
        .await;
        if let Err(progress) = result {
            tracing::warn!(
                target: "aex::session_telemetry",
                event = "sandbox_progress_failed",
                progress,
                session = %session,
            );
        }
    });
}

#[async_trait::async_trait]
trait PreparationLeaseAuthority: Send + Sync {
    async fn renew(
        &self,
        session: aex_wire::ids::SessionId,
        lease: &mut SandboxPreparationLease,
        now: Timestamp,
    ) -> Result<(), SandboxPreparationAuthorityError>;

    async fn release(
        &self,
        session: aex_wire::ids::SessionId,
        lease: &SandboxPreparationLease,
    ) -> Result<(), SandboxPreparationAuthorityError>;
}

#[async_trait::async_trait]
impl PreparationLeaseAuthority for SandboxPreparationAuthority {
    async fn renew(
        &self,
        session: aex_wire::ids::SessionId,
        lease: &mut SandboxPreparationLease,
        now: Timestamp,
    ) -> Result<(), SandboxPreparationAuthorityError> {
        SandboxPreparationAuthority::renew(self, session, lease, now).await
    }

    async fn release(
        &self,
        session: aex_wire::ids::SessionId,
        lease: &SandboxPreparationLease,
    ) -> Result<(), SandboxPreparationAuthorityError> {
        SandboxPreparationAuthority::release(self, session, lease).await
    }
}

struct LeaseHeartbeat {
    authority: Arc<dyn PreparationLeaseAuthority>,
    lease: Arc<tokio::sync::Mutex<SandboxPreparationLease>>,
    stopped: Arc<AtomicBool>,
    lost: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl LeaseHeartbeat {
    fn start(
        authority: Arc<dyn PreparationLeaseAuthority>,
        lease: SandboxPreparationLease,
    ) -> Self {
        let lease = Arc::new(tokio::sync::Mutex::new(lease));
        let stopped = Arc::new(AtomicBool::new(false));
        let lost = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let task_authority = Arc::clone(&authority);
        let task_lease = Arc::clone(&lease);
        let task_stopped = Arc::clone(&stopped);
        let task_lost = Arc::clone(&lost);
        let task_wake = Arc::clone(&wake);
        let task = tokio::spawn(async move {
            loop {
                if task_stopped.load(Ordering::Acquire) {
                    return;
                }
                tokio::select! {
                    () = task_wake.notified() => {
                        if task_stopped.load(Ordering::Acquire) {
                            return;
                        }
                    }
                    () = tokio::time::sleep(std::time::Duration::from_millis(
                        SANDBOX_PREPARATION_HEARTBEAT_MILLIS,
                    )) => {}
                }
                if renew_if_due(task_authority.as_ref(), &task_lease, &task_lost)
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        Self {
            authority,
            lease,
            stopped,
            lost,
            wake,
            task: Some(task),
        }
    }

    async fn pulse(&self) -> Result<(), &'static str> {
        renew_if_due(self.authority.as_ref(), &self.lease, &self.lost).await
    }

    async fn release(mut self) -> Result<(), SandboxPreparationAuthorityError> {
        self.stop();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        let lease = self.lease.lock().await.clone();
        self.authority.release(lease.session, &lease).await
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_one();
    }
}

impl Drop for LeaseHeartbeat {
    fn drop(&mut self) {
        self.stop();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn renew_if_due(
    authority: &dyn PreparationLeaseAuthority,
    lease: &tokio::sync::Mutex<SandboxPreparationLease>,
    lost: &AtomicBool,
) -> Result<(), &'static str> {
    if lost.load(Ordering::Acquire) {
        return Err("sandbox_preparation_lease_lost");
    }
    let Some(now) = wall_clock_now() else {
        lost.store(true, Ordering::Release);
        return Err("reading_preparation_clock");
    };
    let mut lease = lease.lock().await;
    if lost.load(Ordering::Acquire) {
        return Err("sandbox_preparation_lease_lost");
    }
    let renew_at = lease.lease_until.unix_millis().saturating_sub(
        SANDBOX_PREPARATION_LEASE_MILLIS.saturating_sub(
            i64::try_from(SANDBOX_PREPARATION_HEARTBEAT_MILLIS).unwrap_or(i64::MAX),
        ),
    );
    if now.unix_millis() < renew_at {
        return Ok(());
    }
    let session = lease.session;
    if authority.renew(session, &mut lease, now).await.is_err() {
        lost.store(true, Ordering::Release);
        return Err("sandbox_preparation_lease_lost");
    }
    Ok(())
}

async fn eagerly_prepare_sandbox(
    shared: &Shared,
    prepared: &CreatePreparation,
) -> Result<(), &'static str> {
    let generation = prepared.generation.ok_or("missing_generation")?;
    let authority = Arc::new(SandboxPreparationAuthority::new(
        shared.authority.clone(),
        shared.tables.session_authority.clone(),
    ));
    let checkpoint = authority
        .observe(
            prepared.workspace,
            prepared.organization,
            prepared.session,
            generation,
        )
        .await
        .map_err(|_| "observing_sandbox_checkpoint")?;
    if checkpoint != aex_session_domain::SandboxPreparationStatus::Requested {
        return Ok(());
    }
    let owner = uuid::Uuid::now_v7().to_string();
    let claim = authority
        .claim(
            prepared.workspace,
            prepared.organization,
            prepared.session,
            generation,
            &owner,
            wall_clock_now().ok_or("reading_preparation_clock")?,
        )
        .await
        .map_err(|_| "claiming_sandbox_preparation")?;
    let Some(lease) = acquired_lease(claim) else {
        return Ok(());
    };
    let heartbeat = LeaseHeartbeat::start(authority.clone(), lease);
    let result =
        eagerly_prepare_owned(shared, prepared, generation, authority.as_ref(), &heartbeat).await;
    let release = heartbeat.release().await;
    match result {
        Err(reason) => Err(reason),
        Ok(()) => release.map_err(|_| "releasing_sandbox_preparation"),
    }
}

fn acquired_lease(claim: SandboxPreparationClaim) -> Option<SandboxPreparationLease> {
    match claim {
        SandboxPreparationClaim::Acquired(lease) => Some(lease),
        SandboxPreparationClaim::Contended { .. } => None,
    }
}

async fn eagerly_prepare_owned(
    shared: &Shared,
    prepared: &CreatePreparation,
    generation: aex_wire::ids::GenerationId,
    authority: &SandboxPreparationAuthority,
    lease: &LeaseHeartbeat,
) -> Result<(), &'static str> {
    // Completion may have won between the pre-claim observation and the
    // conditional lease write. Re-observe under the lease before the first
    // provider or guest effect so Ready/Suspended is a hard no-rematerialize
    // barrier.
    let checkpoint = authority
        .observe(
            prepared.workspace,
            prepared.organization,
            prepared.session,
            generation,
        )
        .await
        .map_err(|_| "observing_claimed_sandbox_checkpoint")?;
    if checkpoint != aex_session_domain::SandboxPreparationStatus::Requested {
        return Ok(());
    }
    sandbox_progress(prepared, "requested");
    sandbox_progress(prepared, "provisioning");
    lease.pulse().await?;
    let first_ready = shared
        .live_files
        .ensure_ready(prepared.session, generation)
        .await
        .map_err(|_| "provisioning")?;
    lease.pulse().await?;
    if !has_exact_provider_lifetime(&first_ready) {
        return Err("provider_lifetime_validation");
    }

    sandbox_progress(prepared, "materializing_registered_files");
    let content = LeasedContent {
        inner: ContentObjects::new(shared.content_objects.as_ref(), prepared.workspace),
        lease,
    };
    let guest = StartupHands {
        live: shared.live_files.as_ref(),
        lease,
    };
    materialize_selected(&content, &guest, prepared)
        .await
        .map_err(|_| "materializing_registered_files")?;

    lease.pulse().await?;
    let confirmed = shared
        .live_files
        .ensure_ready(prepared.session, generation)
        .await
        .map_err(|_| "confirming_materialized_sandbox")?;
    lease.pulse().await?;
    if confirmed.generation != first_ready.generation
        || confirmed.launched_at != first_ready.launched_at
        || confirmed.expires_at != first_ready.expires_at
    {
        return Err("confirming_materialized_sandbox");
    }
    sandbox_progress(prepared, "workspace_materialized");

    // A durable tool waiter may race eager setup. In that case suspension can
    // be refused and the truthful checkpoint is Ready, not a failed setup.
    lease.pulse().await?;
    let suspended = match shared
        .live_files
        .suspend_ready(prepared.session, generation)
        .await
    {
        Ok(()) => true,
        Err(aex_brain_app::ports::HandsError::Transport { detail, .. })
            if detail.kind == aex_brain_app::ports::ProviderFailureKind::Overloaded =>
        {
            false
        }
        Err(_) => return Err("suspending_prepared_sandbox"),
    };
    lease.pulse().await?;
    let settled_at = if suspended {
        wall_clock_now().unwrap_or(confirmed.observed_at)
    } else {
        confirmed.observed_at
    };
    if suspended {
        sandbox_progress(prepared, "suspended");
    }
    sandbox_progress(prepared, "ready");
    lease.pulse().await?;
    authority
        .settle(
            prepared.workspace,
            prepared.organization,
            prepared.session,
            generation,
            suspended,
            settled_at,
        )
        .await
        .map_err(|_| "publishing_sandbox_checkpoint")?;
    Ok(())
}

fn wall_clock_now() -> Option<Timestamp> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Timestamp::from_unix_millis(millis).ok()
}

fn sandbox_progress(prepared: &CreatePreparation, progress: &'static str) {
    tracing::info!(
        target: "aex::session_telemetry",
        event = "sandbox_progress",
        progress,
        session = %prepared.session,
        generation = %prepared.generation.expect("called only for enabled sandboxes"),
    );
}

async fn recover_publication(
    routes: &Routes,
    command: &CreateSession,
    winner: &CreatePreparation,
    observed_at: Timestamp,
    error: &StoreError,
) -> WireResult<models::Session> {
    match routes
        .shared
        .commands
        .load_receipt(
            command.workspace,
            aex_session_app::CREATE_SCOPE,
            &command.identity,
            observed_at,
        )
        .await
    {
        Ok(Some(stored)) => {
            return replayed(
                replay_session_create_receipt(&stored, command)
                    .map_err(|error| create_app_failure(&error))?,
            );
        }
        Ok(None) => {}
        Err(read_error) => {
            // The final transaction may have committed. Never terminate that
            // generation merely because recovery could not prove the receipt;
            // the caller can safely retry the same key.
            eprintln!(
                "session-stream-api: session create publication failed ({error}) and receipt recovery failed ({read_error})"
            );
            return Err(WireError::new(ErrorCode::CommitOutcomeUnknown));
        }
    }
    if let StoreError::PreconditionFailed { participant, .. } = error
        && *participant == Participant::AUTHZ_PLACEMENT
    {
        compensate(routes, winner, "account paused before publication", error).await;
        return Err(WireError::new(ErrorCode::AccountPaused));
    }
    if matches!(error, StoreError::CommitAmbiguous { .. })
        || matches!(
            error,
            StoreError::PreconditionFailed { participant, .. }
                if *participant == Participant::SESSION_HEAD
                    || *participant == Participant::SESSION_IDEMPOTENCY
        )
    {
        // A missing receipt after an ambiguous commit, or after a public
        // identity condition failed, cannot prove that publication failed.
        return Err(WireError::new(ErrorCode::CommitOutcomeUnknown));
    }
    compensate(routes, winner, "public head publication", error).await;
    Err(WireError::new(ErrorCode::InternalError))
}

fn command(routes: &Routes, request: models::SessionCreateRequest) -> WireResult<CreateSession> {
    let replay = routes.cx.idempotency.as_ref().ok_or_else(|| {
        WireError::new(ErrorCode::InvalidRequest)
            .with_message("this route requires an `Idempotency-Key`")
    })?;
    Ok(CreateSession {
        workspace: routes.cx.auth.workspace_id,
        organization: routes.cx.auth.organization_id,
        identity: IdempotencyIdentity::Key(Box::new(ReplayIdentity {
            principal: routes.cx.auth.principal,
            route: RouteId::SessionCreate,
            key: replay.key.clone(),
            intent: IntentDigest::from_bytes(replay.intent),
        })),
        request,
    })
}

fn private_preparation(
    prepared: &PreparedSessionCreate,
    receipt_key_sha256: &str,
    ids: crate::session::app_ports::RequestIds,
) -> WireResult<CreatePreparation> {
    let root_record = initial_root_record(prepared).map_err(|error| create_app_failure(&error))?;
    Ok(CreatePreparation {
        workspace: prepared.command.workspace,
        organization: prepared.command.organization,
        intent: prepared.command.identity.intent(),
        receipt_key_sha256: receipt_key_sha256.to_owned(),
        coordinator: ids.next_uuid_v7(),
        session: prepared.session,
        root_agent: prepared.root_agent,
        generation: prepared.generation,
        files: prepared
            .initial_files
            .iter()
            .map(|file| {
                Ok(PreparedFile {
                    name: file.name.clone(),
                    revision: file.revision,
                    etag: file.etag.clone(),
                    content: file.value.content.sha256,
                    size_bytes: file
                        .size_bytes()
                        .map_err(|error| create_app_failure(&error))?,
                    mount_path: file.mount_path.clone(),
                    media_type: file.value.media_type.clone(),
                    mode: file.value.mode,
                })
            })
            .collect::<WireResult<Vec<_>>>()?,
        root_record,
        runtime_definition: prepared
            .pinned_runtime
            .as_ref()
            .map(|runtime| to_jcs_bytes(runtime.definition()))
            .transpose()
            .map_err(|_| WireError::new(ErrorCode::InternalError))?,
        resolved_config: prepared.resolved.document().as_bytes().to_vec(),
        metadata: prepared
            .metadata
            .as_ref()
            .map(|metadata| metadata.document().as_bytes().to_vec()),
        materialized_agents: prepared.materialized_agents,
        account_revision: prepared.account_revision.0,
        prepared_at: prepared.prepared_at,
    })
}

fn require_winner_intent(winner: &CreatePreparation, command: &CreateSession) -> WireResult<()> {
    if winner.workspace != command.workspace || winner.organization != command.organization {
        return Err(WireError::new(ErrorCode::InternalError));
    }
    if winner.intent != command.identity.intent() {
        return Err(WireError::new(command.identity.conflict_code()));
    }
    Ok(())
}

fn rehydrate(
    winner: &CreatePreparation,
    command: &CreateSession,
) -> WireResult<PreparedSessionCreate> {
    let pinned_runtime = rehydrate_pinned_runtime(winner)?;
    let resolved_document = std::str::from_utf8(&winner.resolved_config)
        .ok()
        .and_then(|text| CanonicalJson::parse(text).ok())
        .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;
    let resolved_public: models::ResolvedConfig =
        serde_json::from_value(resolved_document.to_value())
            .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let resolved = ResolvedConfigAuthority::new(
        resolved_document,
        resolved_public.provider,
        resolved_public.model,
    )
    .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let metadata = rehydrate_metadata(winner)?;
    let JournalRecord::AgentStarted { config, budget, .. } = &winner.root_record else {
        return Err(WireError::new(ErrorCode::InternalError));
    };
    if config.provider != resolved.provider()
        || config.model.as_str() != resolved.model()
        || config.hands_generation != winner.generation
        || config.limits_revision == 0
    {
        return Err(WireError::new(ErrorCode::InternalError));
    }
    let initial_files = winner
        .files
        .iter()
        .map(|file| ResolvedInitialFile {
            name: file.name.clone(),
            revision: file.revision,
            etag: file.etag.clone(),
            mount_path: file.mount_path.clone(),
            value: models::RegisteredFileRead {
                content: models::ContentRef {
                    sha256: file.content,
                    size_bytes: DecimalU128::new(u128::from(file.size_bytes)),
                },
                media_type: file.media_type.clone(),
                mode: file.mode,
            },
        })
        .collect();
    Ok(PreparedSessionCreate {
        command: command.clone(),
        session: winner.session,
        root_agent: winner.root_agent,
        generation: winner.generation,
        pinned_runtime,
        provider_credential: ProviderCredentialPin {
            credential: config.credential.binding,
            provider: config.provider,
            source_generation: config.credential.generation.get(),
            revision: config.credential.revision.get(),
        },
        resolved,
        limits_revision: config.limits_revision,
        catalog_revision: config.catalog_pin.to_wire(),
        agent_execution: AgentExecutionLimits {
            turn_deadline_ms: config.limits.turn_deadline_ms,
            max_depth: config.limits.max_depth,
            max_fanout: config.limits.max_fanout,
        },
        run_budget: RunBudgetLimits {
            max_run_duration_ms: config.limits.max_run_duration_ms,
            total_children_created: budget.get(Dimension::TotalChildrenCreated),
            provider_calls: budget.get(Dimension::ProviderCalls),
            hands_calls: budget.get(Dimension::HandsCalls),
            queued_children: budget.get(Dimension::QueuedChildren),
            retained_result_bytes: budget.get(Dimension::RetainedResultBytes),
        },
        account_revision: AccountRevision(winner.account_revision),
        materialized_agents: winner.materialized_agents,
        metadata,
        initial_files,
        mcp_servers: config.mcp_servers.clone(),
        prepared_at: winner.prepared_at,
    })
}

fn rehydrate_pinned_runtime(winner: &CreatePreparation) -> WireResult<Option<PinnedRuntime>> {
    winner
        .runtime_definition
        .as_deref()
        .map(|bytes| {
            let definition = serde_json::from_slice(bytes)
                .map_err(|_| WireError::new(ErrorCode::InternalError))?;
            PinnedRuntime::new(
                winner.session,
                winner.workspace,
                winner.organization,
                definition,
            )
            .map_err(|_| WireError::new(ErrorCode::InternalError))
        })
        .transpose()
}

fn rehydrate_metadata(winner: &CreatePreparation) -> WireResult<Option<SessionMetadata>> {
    winner
        .metadata
        .as_deref()
        .map(|bytes| {
            let text =
                std::str::from_utf8(bytes).map_err(|_| WireError::new(ErrorCode::InternalError))?;
            let document =
                CanonicalJson::parse(text).map_err(|_| WireError::new(ErrorCode::InternalError))?;
            SessionMetadata::new(document).map_err(|_| WireError::new(ErrorCode::InternalError))
        })
        .transpose()
}

struct LeasedContent<'a> {
    inner: ContentObjects<'a>,
    lease: &'a LeaseHeartbeat,
}

#[async_trait::async_trait]
impl StartupContent for LeasedContent<'_> {
    async fn read(&self, file: &PreparedFile) -> Result<StartupBody, StartupFileError> {
        self.lease
            .pulse()
            .await
            .map_err(|_| StartupGuestError::Unavailable)?;
        let body = self.inner.read(file).await?;
        self.lease
            .pulse()
            .await
            .map_err(|_| StartupGuestError::Unavailable)?;
        Ok(body)
    }
}

struct StartupHands<'a> {
    live: &'a dyn aex_brain_hands::LiveFileBackend,
    lease: &'a LeaseHeartbeat,
}

impl StartupGuest for StartupHands<'_> {
    async fn call(
        &self,
        session: aex_wire::ids::SessionId,
        generation: aex_wire::ids::GenerationId,
        activity: HandsOperationId,
        requests: Vec<FileRequest>,
    ) -> Result<Vec<FileResponse>, StartupGuestError> {
        self.lease
            .pulse()
            .await
            .map_err(|_| StartupGuestError::Unavailable)?;
        let reply = self
            .live
            .call(session, generation, activity, &requests)
            .await
            .map_err(|_| StartupGuestError::Unavailable)?;
        self.lease
            .pulse()
            .await
            .map_err(|_| StartupGuestError::Unavailable)?;
        if reply.generation != generation {
            return Err(StartupGuestError::Mismatched);
        }
        Ok(reply.responses)
    }
}

fn has_exact_provider_lifetime(ready: &aex_brain_hands::LiveGenerationReady) -> bool {
    let Ok(lifetime_ms) = i64::try_from(aex_runtime_control::PROVIDER_LIFETIME_MS) else {
        return false;
    };
    ready.launched_at.unix_millis().checked_add(lifetime_ms) == Some(ready.expires_at.unix_millis())
}

async fn compensate(
    routes: &Routes,
    winner: &CreatePreparation,
    stage: &'static str,
    cause: &impl std::fmt::Display,
) {
    let Some(generation) = winner.generation else {
        eprintln!(
            "session-stream-api: create failed during {stage} ({cause}); no sandbox generation existed"
        );
        return;
    };
    if let Err(error) = routes
        .shared
        .live_files
        .abort_unpublished(winner.session, generation)
        .await
    {
        eprintln!(
            "session-stream-api: create failed during {stage} ({cause}); exact-generation compensation also failed: {error}"
        );
    } else {
        eprintln!(
            "session-stream-api: create failed during {stage} ({cause}); exact generation was compensated"
        );
    }
}

fn replayed(outcome: PrepareSessionCreateOutcome) -> WireResult<models::Session> {
    match outcome {
        PrepareSessionCreateOutcome::Replayed { session, .. } => Ok(*session),
        PrepareSessionCreateOutcome::Prepared(_) => Err(WireError::new(ErrorCode::InternalError)),
    }
}

fn port_failure(error: aex_session_app::PortError) -> WireError {
    create_app_failure(&aex_session_app::AppError::Port(error))
}

fn create_app_failure(error: &aex_session_app::AppError) -> WireError {
    if matches!(
        error,
        aex_session_app::AppError::Port(aex_session_app::PortError::NotFound { kind: "limit" })
    ) {
        return WireError::new(ErrorCode::WorkspaceActivationRequired);
    }
    WireError::new(error.code())
}

fn preparation_failure(error: &CreatePreparationError) -> WireError {
    match error {
        CreatePreparationError::FileCount { .. } | CreatePreparationError::FileBytes { .. } => {
            WireError::new(ErrorCode::LimitExceeded)
        }
        CreatePreparationError::Store(StoreError::CommitAmbiguous { .. }) => {
            WireError::new(ErrorCode::CommitOutcomeUnknown)
        }
        CreatePreparationError::Store(
            StoreError::PreconditionFailed { .. } | StoreError::Contended,
        ) => WireError::new(ErrorCode::CommitOutcomeUnknown),
        _ => WireError::new(ErrorCode::InternalError),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use aex_wire::ids::{
        GenerationId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
    };

    struct FakeLeaseAuthority {
        renews: AtomicUsize,
        releases: AtomicUsize,
        lose_on_renew: bool,
    }

    #[async_trait::async_trait]
    impl PreparationLeaseAuthority for FakeLeaseAuthority {
        async fn renew(
            &self,
            _session: SessionId,
            _lease: &mut SandboxPreparationLease,
            _now: Timestamp,
        ) -> Result<(), SandboxPreparationAuthorityError> {
            self.renews.fetch_add(1, Ordering::SeqCst);
            if self.lose_on_renew {
                Err(SandboxPreparationAuthorityError::LeaseLost)
            } else {
                Ok(())
            }
        }

        async fn release(
            &self,
            _session: SessionId,
            _lease: &SandboxPreparationLease,
        ) -> Result<(), SandboxPreparationAuthorityError> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn lease(lease_until: Timestamp) -> SandboxPreparationLease {
        SandboxPreparationLease {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            session: SessionId::from_uuid7(Uuid7::compose(1, [3; 10])),
            generation: GenerationId::from_uuid7(Uuid7::compose(1, [4; 10])),
            owner: "eager-owner".to_owned(),
            lease_until,
        }
    }

    #[test]
    fn contended_claim_does_not_select_an_eager_materializer() {
        let lease_until = wall_clock_now().expect("clock");
        assert!(acquired_lease(SandboxPreparationClaim::Contended { lease_until }).is_none());
    }

    #[tokio::test]
    async fn lease_loss_is_sticky_across_future_pulses() {
        let authority = FakeLeaseAuthority {
            renews: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
            lose_on_renew: true,
        };
        let expired = wall_clock_now().expect("clock");
        let lease = tokio::sync::Mutex::new(lease(expired));
        let lost = AtomicBool::new(false);

        assert!(renew_if_due(&authority, &lease, &lost).await.is_err());
        assert!(renew_if_due(&authority, &lease, &lost).await.is_err());
        assert!(lost.load(Ordering::SeqCst));
        assert_eq!(authority.renews.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn release_stops_the_heartbeat_before_releasing_the_claim() {
        let authority = Arc::new(FakeLeaseAuthority {
            renews: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
            lose_on_renew: false,
        });
        let now = wall_clock_now().expect("clock");
        let lease_until = Timestamp::from_unix_millis(
            now.unix_millis()
                .saturating_add(SANDBOX_PREPARATION_LEASE_MILLIS),
        )
        .expect("deadline");
        let heartbeat = LeaseHeartbeat::start(authority.clone(), lease(lease_until));

        heartbeat.release().await.expect("release");

        assert_eq!(authority.renews.load(Ordering::SeqCst), 0);
        assert_eq!(authority.releases.load(Ordering::SeqCst), 1);
    }
}
