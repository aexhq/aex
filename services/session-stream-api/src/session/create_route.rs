//! Production synchronous session-create orchestration.
//!
//! The public session does not exist while this module launches compute. A
//! receipt-keyed private preparation first elects every identity and selected
//! file revision. Only after the exact generation is reachable, all bytes are
//! materialized, and root `AgentStarted` is durable does one final transaction
//! publish the head and exact response receipt.

use std::future::Future;

use aex_brain_domain::budget::Dimension;
use aex_brain_domain::journal::JournalRecord;
use aex_hands_protocol::files::{FileRequest, FileResponse};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_session_app::ports::{
    AgentExecutionLimits, IdFactory as _, RunBudgetLimits, SessionReader as _,
};
use aex_session_app::{
    CreateSession, PrepareSessionCreateOutcome, PreparedSessionCreate, ReadySessionLaunch,
    ResolvedInitialFile, RootStartedEvidence, initial_root_record, prepare_session_create,
    publish_ready_session, replay_session_create_receipt,
};
use aex_session_domain::{
    AccountRevision, AgentRevision, IdempotencyIdentity, JournalSeq, PinnedRuntime,
    ProviderCredentialPin, ReceiptKey, ResolvedConfigAuthority, SessionMetadata,
};
use aex_session_dynamodb::app_authority::{ApiHintSink, DynamoAuthorityCommitter};
use aex_session_dynamodb::application_plan::{FamilyCompilers, SessionBinding};
use aex_session_dynamodb::create_preparation::{
    CreatePreparation, CreatePreparationError, CreatePreparationStore, PreparedFile, RootStarted,
};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::Participant;
use aex_wire::canonical::{CanonicalJson, to_jcs_bytes};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::{IntentDigest, ReplayIdentity};
use aex_wire::models;
use aex_wire::routes::RouteId;
use aex_wire::types::DecimalU128;

use super::create_readiness::{
    ContentObjects, StartupGuest, StartupGuestError, materialize_selected,
};
use super::handlers::Routes;

/// Runs the only public session activation path to a truthful ready `201`.
pub(super) async fn create_session(
    routes: &Routes,
    request: models::SessionCreateRequest,
) -> WireResult<models::Session> {
    let command = command(routes, request)?;
    let receipt_key = ReceiptKey::of(aex_session_app::CREATE_SCOPE, &command.identity)
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let bindings = routes.bindings()?;
    let now = routes.now()?;

    // Receipt before both private election and every volatile planning read.
    // An accepted retry remains independent of a later pause, credential
    // revocation, registry deletion or model-catalog change.
    if let Some(stored) = routes
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
    {
        return replayed(replay_session_create_receipt(&stored, &command)?);
    }

    let preparations = CreatePreparationStore::new(
        routes.shared.authority.clone(),
        routes.shared.tables.session_authority.clone(),
    );
    let prepared = if let Some(winner) = preparations
        .load(command.workspace, receipt_key.key_sha256())
        .await
        .map_err(preparation_failure)?
    {
        require_winner_intent(&winner, &command)?;
        rehydrate(&winner, &command)?
    } else {
        let context = bindings.context();
        match prepare_session_create(&context, &command)
            .await
            .map_err(create_app_failure)?
        {
            PrepareSessionCreateOutcome::Replayed { session, .. } => return Ok(session),
            PrepareSessionCreateOutcome::Prepared(candidate) => {
                let candidate = private_preparation(
                    candidate.as_ref(),
                    receipt_key.key_sha256(),
                    bindings.ids(),
                )?;
                let winner = preparations
                    .stage_and_elect(&candidate)
                    .await
                    .map_err(preparation_failure)?;
                require_winner_intent(&winner, &command)?;
                rehydrate(&winner, &command)?
            }
        }
    };
    let winner = preparations
        .load(command.workspace, receipt_key.key_sha256())
        .await
        .map_err(preparation_failure)?
        .ok_or_else(|| WireError::new(ErrorCode::InternalError))?;

    let definition = prepared.pinned_runtime.definition();
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

    let first_ready = match routes
        .shared
        .live_files
        .ensure_ready(winner.session, winner.generation)
        .await
    {
        Ok(ready) => ready,
        Err(error) => {
            compensate(routes, &winner, "launch readiness", &error).await;
            return Err(WireError::new(ErrorCode::InternalError));
        }
    };
    if !has_exact_provider_lifetime(&first_ready) {
        compensate(
            routes,
            &winner,
            "provider lifetime validation",
            &"the ready generation does not carry the fixed eight-hour fence",
        )
        .await;
        return Err(WireError::new(ErrorCode::InternalError));
    }
    let content = ContentObjects::new(routes.shared.content_objects.as_ref(), winner.workspace);
    let guest = StartupHands(routes.shared.live_files.as_ref());
    let materialized = match materialize_selected(&content, &guest, &winner).await {
        Ok(materialized) => materialized,
        Err(error) => {
            compensate(routes, &winner, "startup file materialization", &error).await;
            return Err(WireError::new(ErrorCode::InternalError));
        }
    };
    // A final authenticated observation proves that the generation remained
    // reachable after the last guest rename. Its observation time is the
    // truthful occurrence time for the durable root start.
    let ready = match routes
        .shared
        .live_files
        .ensure_ready(winner.session, winner.generation)
        .await
    {
        Ok(ready)
            if ready.generation == first_ready.generation
                && ready.launched_at == first_ready.launched_at
                && ready.expires_at == first_ready.expires_at =>
        {
            ready
        }
        Ok(_) => {
            compensate(
                routes,
                &winner,
                "readiness identity drift",
                &"mismatched readiness",
            )
            .await;
            return Err(WireError::new(ErrorCode::InternalError));
        }
        Err(error) => {
            compensate(routes, &winner, "final readiness", &error).await;
            return Err(WireError::new(ErrorCode::InternalError));
        }
    };
    let root = match preparations
        .commit_root_started(
            &winner,
            RootStarted {
                occurred_at: ready.observed_at,
            },
        )
        .await
    {
        Ok(root) => root,
        Err(error) => {
            compensate(routes, &winner, "root AgentStarted", &error).await;
            return Err(preparation_failure(error));
        }
    };
    let readiness = ReadySessionLaunch {
        generation: winner.generation,
        launched_at: ready.launched_at,
        materialized_files: materialized
            .into_iter()
            .map(|file| (file.name, file.revision))
            .collect(),
        root_started: RootStartedEvidence {
            agent: winner.root_agent,
            session: winner.session,
            generation: winner.generation,
            occurred_at: root.occurred_at,
            revision: AgentRevision(root.revision),
            journal_tail: JournalSeq(root.journal_tail),
            journal_tail_hash: root.journal_tail_hash,
        },
    };
    let planned = match publish_ready_session(&prepared, &readiness) {
        Ok(planned) => planned,
        Err(error) => {
            compensate(routes, &winner, "ready publication planning", &error).await;
            return Err(create_app_failure(error));
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
        ready.observed_at,
        ApiHintSink,
    );
    if let Err(error) = committer
        .commit_replayable(&planned.plan, &FamilyCompilers::new())
        .await
    {
        match routes
            .shared
            .commands
            .load_receipt(
                command.workspace,
                aex_session_app::CREATE_SCOPE,
                &command.identity,
                ready.observed_at,
            )
            .await
        {
            Ok(Some(stored)) => {
                return replayed(replay_session_create_receipt(&stored, &command)?);
            }
            Ok(None) => {}
            Err(read_error) => {
                // The final transaction may have committed. Never terminate
                // that generation merely because recovery could not prove the
                // receipt; the caller can safely retry the same key.
                eprintln!(
                    "session-stream-api: session create publication failed ({error}) and receipt recovery failed ({read_error})"
                );
                return Err(WireError::new(ErrorCode::CommitOutcomeUnknown));
            }
        }
        if let StoreError::PreconditionFailed { participant, .. } = &error
            && *participant == Participant::AUTHZ_PLACEMENT
        {
            compensate(routes, &winner, "account paused before publication", &error).await;
            return Err(WireError::new(ErrorCode::AccountPaused));
        }
        if matches!(error, StoreError::CommitAmbiguous { .. })
            || matches!(
                &error,
                StoreError::PreconditionFailed { participant, .. }
                    if *participant == Participant::SESSION_HEAD
                        || *participant == Participant::SESSION_IDEMPOTENCY
            )
        {
            // A missing receipt immediately after an ambiguous multi-table
            // commit, or after a condition says a public identity already
            // exists, is not proof of rejection. Preserve the exact generation
            // for receipt-first retry/reconciliation instead of destroying a
            // potentially published session.
            return Err(WireError::new(ErrorCode::CommitOutcomeUnknown));
        }
        compensate(routes, &winner, "public head publication", &error).await;
        return Err(WireError::new(ErrorCode::InternalError));
    }
    aex_session_app::public_session(&planned.projected)
        .map_err(|_| WireError::new(ErrorCode::InternalError))
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
    ids: &crate::session::app_ports::RequestIds,
) -> WireResult<CreatePreparation> {
    let root_record = initial_root_record(prepared).map_err(create_app_failure)?;
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
                    size_bytes: file.size_bytes().map_err(create_app_failure)?,
                    mount_path: file.value.mount_path.clone(),
                    media_type: file.value.media_type.clone(),
                    mode: file.value.mode,
                })
            })
            .collect::<WireResult<Vec<_>>>()?,
        root_record,
        runtime_definition: to_jcs_bytes(prepared.pinned_runtime.definition())
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
    let definition = serde_json::from_slice(winner.runtime_definition.as_slice())
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
    let pinned_runtime = PinnedRuntime::new(
        winner.session,
        winner.workspace,
        winner.organization,
        definition,
    )
    .map_err(|_| WireError::new(ErrorCode::InternalError))?;
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
    let metadata = winner
        .metadata
        .as_deref()
        .map(|bytes| {
            let text =
                std::str::from_utf8(bytes).map_err(|_| WireError::new(ErrorCode::InternalError))?;
            let document =
                CanonicalJson::parse(text).map_err(|_| WireError::new(ErrorCode::InternalError))?;
            SessionMetadata::new(document).map_err(|_| WireError::new(ErrorCode::InternalError))
        })
        .transpose()?;
    let JournalRecord::AgentStarted { config, budget, .. } = &winner.root_record else {
        return Err(WireError::new(ErrorCode::InternalError));
    };
    if config.provider != resolved.provider()
        || config.model.as_str() != resolved.model()
        || config.catalog_pin.to_wire() != resolved_public.catalog_revision
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
            value: models::RegisteredFileRead {
                content: models::ContentRef {
                    sha256: file.content,
                    size_bytes: DecimalU128::new(u128::from(file.size_bytes)),
                },
                media_type: file.media_type.clone(),
                mode: file.mode,
                mount_path: file.mount_path.clone(),
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
        agent_execution: AgentExecutionLimits {
            max_turns: config.limits.max_turns,
            max_steps_per_turn: config.limits.max_steps_per_turn,
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
        prepared_at: winner.prepared_at,
    })
}

struct StartupHands<'a>(&'a dyn aex_brain_hands::LiveFileBackend);

impl StartupGuest for StartupHands<'_> {
    fn call(
        &self,
        session: aex_wire::ids::SessionId,
        generation: aex_wire::ids::GenerationId,
        activity: HandsOperationId,
        requests: Vec<FileRequest>,
    ) -> impl Future<Output = Result<Vec<FileResponse>, StartupGuestError>> + Send {
        async move {
            let reply = self
                .0
                .call(session, generation, activity, &requests)
                .await
                .map_err(|_| StartupGuestError::Unavailable)?;
            if reply.generation != generation {
                return Err(StartupGuestError::Mismatched);
            }
            Ok(reply.responses)
        }
    }
}

async fn compensate(
    routes: &Routes,
    winner: &CreatePreparation,
    stage: &'static str,
    cause: &impl std::fmt::Display,
) {
    if let Err(error) = routes
        .shared
        .live_files
        .abort_unpublished(winner.session, winner.generation)
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
        PrepareSessionCreateOutcome::Replayed { session, .. } => Ok(session),
        PrepareSessionCreateOutcome::Prepared(_) => Err(WireError::new(ErrorCode::InternalError)),
    }
}

fn port_failure(error: aex_session_app::PortError) -> WireError {
    create_app_failure(aex_session_app::AppError::Port(error))
}

fn create_app_failure(error: aex_session_app::AppError) -> WireError {
    if matches!(
        &error,
        aex_session_app::AppError::Port(aex_session_app::PortError::NotFound { kind: "limit" })
    ) {
        return WireError::new(ErrorCode::WorkspaceActivationRequired);
    }
    WireError::new(error.code())
}

fn preparation_failure(error: CreatePreparationError) -> WireError {
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

fn has_exact_provider_lifetime(ready: &aex_brain_hands::LiveGenerationReady) -> bool {
    let Ok(lifetime_ms) = i64::try_from(aex_runtime_control::PROVIDER_LIFETIME_MS) else {
        return false;
    };
    ready.launched_at.unix_millis().checked_add(lifetime_ms) == Some(ready.expires_at.unix_millis())
}

#[cfg(test)]
mod tests {
    use aex_brain_hands::LiveGenerationReady;
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;

    use super::has_exact_provider_lifetime;

    fn ready(expires_at_ms: i64) -> LiveGenerationReady {
        LiveGenerationReady {
            generation: GenerationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            launched_at: Timestamp::from_unix_millis(1_000).expect("launch"),
            expires_at: Timestamp::from_unix_millis(expires_at_ms).expect("expiry"),
            observed_at: Timestamp::from_unix_millis(2_000).expect("observation"),
            lifecycle_fence: 1,
        }
    }

    #[test]
    fn create_accepts_only_the_native_eight_hour_provider_fence() {
        assert!(has_exact_provider_lifetime(&ready(28_801_000)));
        assert!(!has_exact_provider_lifetime(&ready(28_800_999)));
        assert!(!has_exact_provider_lifetime(&ready(28_801_001)));
    }
}
